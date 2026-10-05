//! Responses 的完整快照和增量诊断共用字段路径，不再次投影快照正文。
use crate::{
    adapter::protocol_codec::projection::{self, Notes},
    protocol::responses::response::{
        Event,
        body::OutputItem,
        event::{ContentPart, KnownEvent as E, ReasoningPart},
        message::ContentPart as OutputPart,
    },
};

/// 所有已知事件的扩展都诊断；未知事件整体交给未知事件策略。
pub(super) fn collect(event: &Event, notes: &mut Notes) {
    let Event::Known(event) = event else {
        return;
    };
    notes.extra(event.extra(), &format!("events.{}", event.kind()));
    match event.as_ref() {
        E::Created(lifecycle)
        | E::InProgress(lifecycle)
        | E::Queued(lifecycle)
        | E::Completed(lifecycle)
        | E::Incomplete(lifecycle)
        | E::Failed(lifecycle) => {
            let body = &lifecycle.response;
            projection::responses_fields(notes, body);
            if let Some(error) = body.error.as_option() {
                notes.field(&error.misalignment, "response.error.misalignment");
                notes.extra(&error.extra, "response.error");
            }
            if let Some(details) = body.incomplete_details.as_option() {
                notes.extra(&details.extra, "response.incomplete_details");
            }
            for (i, output) in body.output.iter().enumerate() {
                item(output, notes, &format!("output[{i}]"));
            }
        }
        E::OutputItemAdded(output) | E::OutputItemDone(output) => item(&output.item, notes, "item"),
        E::ContentPartAdded(part) | E::ContentPartDone(part) => match &part.part {
            ContentPart::Output(part) => content(part, notes, "part"),
            ContentPart::Reasoning(part) => reasoning(part, notes, "part"),
        },
        E::ReasoningSummaryPartAdded(part) | E::ReasoningSummaryPartDone(part) => {
            reasoning(&part.part, notes, "part")
        }
        E::OutputTextDelta(delta) | E::RefusalDelta(delta) | E::ReasoningTextDelta(delta) => {
            notes.field(&delta.logprobs, "delta.logprobs");
            notes.field(&delta.obfuscation, "delta.obfuscation");
        }
        E::OutputTextDone(done) | E::ReasoningTextDone(done) => {
            notes.field(&done.logprobs, "done.logprobs")
        }
        E::FunctionArgumentsDelta(delta)
        | E::CustomToolInputDelta(delta)
        | E::McpArgumentsDelta(delta)
        | E::CodeInterpreterCodeDelta(delta) => {
            notes.field(&delta.obfuscation, "delta.obfuscation")
        }
        E::ReasoningSummaryTextDelta(delta) => notes.field(&delta.obfuscation, "delta.obfuscation"),
        E::ShellCommandDelta(delta) => notes.field(&delta.obfuscation, "delta.obfuscation"),
        E::ShellOutputDelta(delta) => {
            notes.extra(&delta.delta.extra, "delta");
        }
        E::ShellOutputDone(done) => {
            for (i, output) in done.output.iter().enumerate() {
                let path = format!("output[{i}]");
                notes.extra(&output.extra, &path);
                notes.field(&output.created_by, &format!("{path}.created_by"));
                let extra = match &output.outcome {
                    crate::protocol::responses::response::event::ShellOutcome::Exit {
                        extra,
                        ..
                    }
                    | crate::protocol::responses::response::event::ShellOutcome::Timeout {
                        extra,
                    } => extra,
                };
                notes.extra(extra, &format!("{path}.outcome"));
            }
        }
        E::Error(error) => notes.field(&error.param, "error.param"),
        _ => {}
    }
}

/// 输出项的来源 ID 由目标重新分配；未解释的专属字段仍明确报告。
fn item(item: &OutputItem, notes: &mut Notes, path: &str) {
    match item {
        OutputItem::FunctionCall(call) => notes.extra(&call.extra, path),
        OutputItem::Message(message) => {
            notes.extra(&message.extra, path);
            notes.field(&message.phase, &format!("{path}.phase"));
            for (i, part) in message.content.iter().enumerate() {
                content(part, notes, &format!("{path}.content[{i}]"));
            }
        }
        OutputItem::Other(raw) => {
            let kind = raw
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let known: &[&str] = match kind {
                "reasoning" => &["type", "id", "summary"],
                "custom_tool_call" => &["type", "id", "call_id", "name", "input", "status"],
                "code_interpreter_call" => {
                    &["type", "id", "code", "outputs", "container_id", "status"]
                }
                "web_search_call" => &["type", "id", "action", "status"],
                "file_search_call" => &["type", "id", "queries", "results", "status"],
                "mcp_call" => &[
                    "type",
                    "id",
                    "arguments",
                    "name",
                    "output",
                    "error",
                    "server_label",
                    "approval_request_id",
                    "status",
                ],
                "mcp_list_tools" => &["type", "id", "server_label", "tools", "error"],
                "shell_call_output" => &[
                    "type",
                    "id",
                    "call_id",
                    "output",
                    "max_output_length",
                    "status",
                ],
                _ => &[],
            };
            notes.object_extra(raw, known, path);
            if kind == "reasoning" {
                for (i, part) in raw
                    .get("summary")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(part) = part.as_object() {
                        notes.object_extra(
                            part,
                            &["type", "text"],
                            &format!("{path}.summary[{i}]"),
                        );
                    }
                }
            }
        }
    }
}

/// 对数概率没有公共目标表示；引用则由 IR 的独立 Annotation 事件传递。
fn content(part: &OutputPart, notes: &mut Notes, path: &str) {
    let extra = match part {
        OutputPart::OutputText {
            logprobs, extra, ..
        } => {
            notes.field(logprobs, &format!("{path}.logprobs"));
            extra
        }
        OutputPart::Refusal { extra, .. } => extra,
    };
    notes.extra(extra, path);
}

/// 思考正文和摘要均保留可见文本，附属元数据不复制到其他 Provider。
fn reasoning(part: &ReasoningPart, notes: &mut Notes, path: &str) {
    let extra = match part {
        ReasoningPart::ReasoningText { extra, .. } | ReasoningPart::SummaryText { extra, .. } => {
            extra
        }
    };
    notes.extra(extra, path);
}
