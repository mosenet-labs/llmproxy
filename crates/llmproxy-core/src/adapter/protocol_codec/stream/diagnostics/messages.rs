//! Messages 的块、局部统计和停止元数据各自诊断。
use crate::{
    adapter::protocol_codec::projection::{self, Notes},
    protocol::messages::response::{
        Event,
        event::{Delta, KnownDelta, KnownEvent as E},
        message::{ContentBlock, KnownContentBlock as B},
    },
};

/// 容器和调用来源不能通过复制参数伪装成普通客户端函数。
pub(super) fn collect(event: &Event, notes: &mut Notes) {
    let Event::Known(event) = event else {
        return;
    };
    let path = format!("events.{}", event.kind());
    notes.extra(event.extra(), &path);
    match event.as_ref() {
        E::MessageStart(start) => {
            let body = &start.message;
            notes.extra(&body.extra, "response");
            notes.field(&body.container, "response.container");
            notes.field(&body.diagnostics, "response.diagnostics");
            notes.field(&body.stop_details, "response.stop_details");
            notes.field(&body.stop_sequence, "response.stop_sequence");
            projection::messages_usage(notes, &body.usage);
            for (i, block) in body.content.iter().enumerate() {
                content(block, notes, &format!("content[{i}]"));
            }
        }
        E::ContentBlockStart(start) => content(&start.content_block, notes, "content_block"),
        E::ContentBlockDelta(delta) => {
            if let Delta::Known(delta) = &delta.delta {
                let extra = match delta.as_ref() {
                    KnownDelta::TextDelta { extra, .. }
                    | KnownDelta::InputJsonDelta { extra, .. }
                    | KnownDelta::ThinkingDelta { extra, .. }
                    | KnownDelta::SignatureDelta { extra, .. }
                    | KnownDelta::CitationsDelta { extra, .. } => extra,
                };
                notes.extra(extra, "delta");
            }
        }
        E::MessageDelta(delta) => {
            notes.extra(&delta.delta.extra, "delta");
            notes.field(&delta.delta.stop_sequence, "delta.stop_sequence");
            if let Some(usage) = delta.usage.as_option() {
                notes.extra(&usage.extra, "usage");
                notes.field(&usage.server_tool_use, "usage.server_tool_use");
                if let Some(details) = usage.cache_creation.as_option() {
                    notes.extra(&details.extra, "usage.cache_creation");
                }
                if let Some(details) = usage.output_tokens_details.as_option() {
                    notes.extra(&details.extra, "usage.output_tokens_details");
                }
            }
        }
        E::Error(error) => notes.extra(&error.error.extra, "error"),
        _ => {}
    }
}

/// 已知服务端结果只取可见正文，扩展仍逐字段报告。
fn content(block: &ContentBlock, notes: &mut Notes, path: &str) {
    match block {
        ContentBlock::Known(block) => {
            let extra = match block {
                B::Text { extra, .. }
                | B::Thinking { extra, .. }
                | B::RedactedThinking { extra, .. } => extra,
                B::ToolUse { caller, extra, .. } => {
                    if caller.as_option().is_some_and(|caller| {
                        caller.get("type").and_then(serde_json::Value::as_str) != Some("direct")
                    }) {
                        notes.reject(
                            &format!("{path}.caller"),
                            "程序化工具调用依赖来源容器，不能转换为普通客户端函数",
                        );
                    }
                    notes.field(caller, &format!("{path}.caller"));
                    extra
                }
            };
            notes.extra(extra, path);
        }
        ContentBlock::Other(raw) => notes.object_extra(
            raw,
            &["type", "id", "name", "input", "tool_use_id", "content"],
            path,
        ),
    }
}
