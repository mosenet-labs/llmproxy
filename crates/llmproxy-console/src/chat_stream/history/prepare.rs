//! 从完整历史构造本轮输入，工具配对和来源私有状态在此检查。
use super::tools::{self, Position};
use super::{Record, Selection};
use llmproxy_core::{
    adapter::protocol_codec::{Conversion, ConversionWarning},
    ir::{
        media::MediaSource,
        message::{Part, PartKind},
        request::{Item, Request},
    },
};
use std::collections::HashMap;

/// 只对发送副本清理私有元数据，原始 IR 与页面展示保持完整。
pub(super) fn request(records: &[Record], target: &Selection) -> Conversion<Request> {
    let mut warnings = Vec::new();
    let paired = tools::prepare(records, target, &mut warnings);
    let mut body = Request::new(target.protocol);
    for (r, record) in records.iter().enumerate() {
        for (i, item) in record.content.items.iter().enumerate() {
            let position = (r, i, 0);
            let item = match item {
                Item::Message(index) => {
                    let Some(original) = record.content.messages.get(*index) else {
                        continue;
                    };
                    let mut message = original.clone();
                    warn_metadata(&message.metadata, record, target, position, &mut warnings);
                    message.metadata.clear();
                    message.parts = original
                        .parts
                        .iter()
                        .enumerate()
                        .filter_map(|(p, part)| {
                            prepare_part(part, (r, i, p), record, target, &paired, &mut warnings)
                        })
                        .collect();
                    if message.parts.is_empty() {
                        continue;
                    }
                    let index = body.messages.len();
                    body.messages.push(message);
                    Item::Message(index)
                }
                Item::ToolCall { .. } | Item::ToolResult(_) => {
                    match paired.get(&position).map(|part| &part.kind) {
                        Some(PartKind::ToolCall(call)) => Item::ToolCall {
                            call: call.clone(),
                            item_id: None,
                        },
                        Some(PartKind::ToolResult(_)) => {
                            let index = body.messages.len();
                            body.messages.push(llmproxy_core::ir::request::Message {
                                role: llmproxy_core::ir::message::Role::Tool,
                                parts: vec![paired[&position].clone()],
                                metadata: Default::default(),
                            });
                            Item::Message(index)
                        }
                        _ => continue,
                    }
                }
                Item::Reasoning(_) => continue,
                Item::Opaque(_) => {
                    warn(
                        &mut warnings,
                        record,
                        target,
                        position,
                        "不透明历史仅保留展示，未发送到当前模型",
                    );
                    continue;
                }
                other => other.clone(),
            };
            body.items.push(item);
        }
    }
    Conversion { body, warnings }
}

/// 可见思考不作为普通上下文；私有文件和不透明块只留在原历史中。
fn prepare_part(
    part: &Part,
    position: Position,
    record: &Record,
    target: &Selection,
    paired: &HashMap<Position, Part>,
    warnings: &mut Vec<ConversionWarning>,
) -> Option<Part> {
    match &part.kind {
        PartKind::Reasoning(_) => return None,
        PartKind::ToolCall(_) | PartKind::ToolResult(_) => return paired.get(&position).cloned(),
        PartKind::Opaque(_) => {
            warn(
                warnings,
                record,
                target,
                position,
                "不透明历史仅保留展示，未发送到当前模型",
            );
            return None;
        }
        PartKind::Media(media)
            if matches!(media.source, MediaSource::FileId(_)) && record.selection != *target =>
        {
            warn(
                warnings,
                record,
                target,
                position,
                "私有文件属于原模型，未发送到当前模型",
            );
            return None;
        }
        _ => {}
    }
    let mut part = part.clone();
    warn_metadata(&part.metadata, record, target, position, warnings);
    part.metadata.clear();
    Some(part)
}

/// 协议外壳不影响通用历史；实际扩展字段被移除时给出不含原值的提示。
fn warn_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
    record: &Record,
    target: &Selection,
    position: Position,
    warnings: &mut Vec<ConversionWarning>,
) {
    if metadata.iter().any(|(key, value)| {
        if key == "_llmproxy_wire" {
            value
                .get("extra")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|extra| {
                    extra.iter().any(|(key, value)| {
                        // 输出项 ID、状态及空注释只是响应外壳，不属于下一轮提示语义。
                        !matches!(key.as_str(), "id" | "status")
                            && !value.is_null()
                            && !value.as_array().is_some_and(Vec::is_empty)
                    })
                })
        } else {
            !value.is_null()
        }
    }) {
        warn(
            warnings,
            record,
            target,
            position,
            "历史的协议扩展字段未作为通用上下文发送",
        );
    }
}

/// 提示只含历史位置与静态原因，避免把参数、签名或文件 ID 写入日志。
pub(super) fn warn(
    warnings: &mut Vec<ConversionWarning>,
    record: &Record,
    target: &Selection,
    position: Position,
    reason: &str,
) {
    warnings.push(ConversionWarning {
        source: record.selection.protocol,
        target: target.protocol,
        path: format!(
            "history[{}].items[{}].parts[{}]",
            position.0, position.1, position.2
        ),
        reason: reason.into(),
    });
}
