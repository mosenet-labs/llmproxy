//! 工具配对后复用整体 Codec 规范化结果数组和错误标志，避免页面重复协议规则。
use super::{Record, Selection, prepare::warn};
use llmproxy_core::{
    adapter::protocol_codec::{ConversionWarning, EncodeMode, ProtocolCodec, RequestTarget},
    ir::{
        message::{Part, PartKind, Role, TOOL_CONTINUATION_ID_PREFIX, ToolCall, ToolResult},
        request::{Item, Message, Request},
    },
};
use std::collections::HashMap;

pub(super) type Position = (usize, usize, usize);
enum Tool<'a> {
    Call(&'a ToolCall),
    Result(&'a ToolResult),
}
struct Entry<'a> {
    position: Position,
    record: &'a Record,
    tool: Tool<'a>,
    part: Part,
}

/// 已配对且来源作用域有效的调用和结果共同保留；不制造执行结果。
pub(super) fn prepare(
    records: &[Record],
    target: &Selection,
    warnings: &mut Vec<ConversionWarning>,
) -> HashMap<Position, Part> {
    let mut entries = Vec::new();
    for (r, record) in records.iter().enumerate() {
        for (i, item) in record.content.items.iter().enumerate() {
            let mut add = |p, tool, part| {
                entries.push(Entry {
                    position: (r, i, p),
                    record,
                    tool,
                    part,
                })
            };
            match item {
                Item::ToolCall { call, .. } => add(
                    0,
                    Tool::Call(call),
                    Part {
                        kind: PartKind::ToolCall(call.clone()),
                        metadata: Default::default(),
                    },
                ),
                Item::ToolResult(result) => add(
                    0,
                    Tool::Result(result),
                    Part {
                        kind: PartKind::ToolResult(result.clone()),
                        metadata: Default::default(),
                    },
                ),
                Item::Message(index) => {
                    if let Some(message) = record.content.messages.get(*index) {
                        for (p, part) in message.parts.iter().enumerate() {
                            match &part.kind {
                                PartKind::ToolCall(call) => add(p, Tool::Call(call), part.clone()),
                                PartKind::ToolResult(result) => {
                                    add(p, Tool::Result(result), part.clone())
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut counts = HashMap::new();
    for entry in &entries {
        if let Tool::Call(call) = &entry.tool
            && let Some(id) = &call.id
        {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    let mut open: Vec<&Entry<'_>> = Vec::new();
    let mut paired = HashMap::new();
    for entry in &entries {
        match &entry.tool {
            Tool::Call(call) => {
                if call
                    .id
                    .as_ref()
                    .is_some_and(|id| counts.get(id).copied().unwrap_or(0) > 1)
                {
                    warn(
                        warnings,
                        entry.record,
                        target,
                        entry.position,
                        "工具调用 ID 重复，已跳过该工具上下文",
                    );
                } else if entry.record.selection != *target
                    && (entry
                        .part
                        .metadata
                        .get("_llmproxy_wire")
                        .and_then(|wire| wire.get("extra"))
                        .and_then(|extra| extra.get("thoughtSignature"))
                        .is_some_and(|value| !value.is_null())
                        || call
                            .id
                            .as_ref()
                            .is_some_and(|id| id.starts_with(TOOL_CONTINUATION_ID_PREFIX)))
                {
                    warn(
                        warnings,
                        entry.record,
                        target,
                        entry.position,
                        "工具签名或续传引用属于原模型，已跳过该工具上下文",
                    );
                } else {
                    open.push(entry);
                }
            }
            Tool::Result(result) => {
                let found = open.iter().position(|entry| {
                    let Tool::Call(call) = &entry.tool else {
                        return false;
                    };
                    result.id.as_ref().map_or_else(
                        || result.name.as_ref() == Some(&call.name),
                        |id| call.id.as_ref() == Some(id),
                    ) && result.name.as_ref().is_none_or(|name| *name == call.name)
                });
                if let Some(index) = found {
                    let call = open.remove(index);
                    if let Some((call_part, result_part)) = normalize(call, entry, target, warnings)
                    {
                        paired.insert(call.position, call_part);
                        paired.insert(entry.position, result_part);
                    }
                } else {
                    warn(
                        warnings,
                        entry.record,
                        target,
                        entry.position,
                        "工具结果没有有效的前置调用，已跳过该工具上下文",
                    );
                }
            }
        }
    }
    for entry in open {
        warn(
            warnings,
            entry.record,
            target,
            entry.position,
            "工具调用尚未取得结果，已跳过该工具上下文",
        );
    }
    paired
}

/// 只转换一个完整工具回合，目标 IR 的数组与元数据随后按目标协议解释。
fn normalize(
    call: &Entry<'_>,
    result: &Entry<'_>,
    target: &Selection,
    warnings: &mut Vec<ConversionWarning>,
) -> Option<(Part, Part)> {
    let mut call_part = call.part.clone();
    let mut result_part = result.part.clone();
    let PartKind::ToolCall(tool) = &mut call_part.kind else {
        unreachable!()
    };
    let PartKind::ToolResult(output) = &mut result_part.kind else {
        unreachable!()
    };
    if result.record.selection.protocol != call.record.selection.protocol
        && (output.content.is_array() || !result_part.metadata.is_empty())
    {
        warn(
            warnings,
            result.record,
            target,
            result.position,
            "工具结果来源与调用不一致，已跳过该工具上下文",
        );
        return None;
    }
    if tool.id.is_none() {
        tool.id = Some(format!(
            "console-history-{}-{}-{}",
            call.position.0, call.position.1, call.position.2
        ));
    }
    output.id.clone_from(&tool.id);
    output.name = Some(tool.name.clone());
    let mut request = Request::new(call.record.selection.protocol);
    request.generation.max_output_tokens = Some(2048);
    request.messages = vec![
        Message {
            role: Role::Assistant,
            parts: vec![call_part],
            metadata: Default::default(),
        },
        Message {
            role: Role::Tool,
            parts: vec![result_part],
            metadata: Default::default(),
        },
    ];
    request.items = vec![Item::Message(0), Item::Message(1)];
    let converted = match target.protocol.encode_request(
        &request,
        &RequestTarget {
            model: "history",
            max_output_tokens: None,
        },
        EncodeMode::Rebuild,
    ) {
        Ok(converted) => converted,
        Err(_) => {
            warn(
                warnings,
                call.record,
                target,
                call.position,
                "当前协议无法表达该工具回合，已跳过该工具上下文",
            );
            return None;
        }
    };
    for mut warning in converted.warnings {
        warning.path = format!(
            "history[{}].items[{}].{}",
            call.position.0, call.position.1, warning.path
        );
        warnings.push(warning);
    }
    let Ok(decoded) = target.protocol.decode_request(&converted.body) else {
        warn(
            warnings,
            call.record,
            target,
            call.position,
            "工具回合无法恢复为目标历史，已跳过该工具上下文",
        );
        return None;
    };
    let mut call_part = None;
    let mut result_part = None;
    for item in decoded.items {
        match item {
            Item::ToolCall { call, .. } => {
                call_part = Some(Part {
                    kind: PartKind::ToolCall(call),
                    metadata: Default::default(),
                })
            }
            Item::ToolResult(result) => {
                result_part = Some(Part {
                    kind: PartKind::ToolResult(result),
                    metadata: Default::default(),
                })
            }
            Item::Message(index) => {
                if let Some(message) = decoded.messages.get(index) {
                    for part in &message.parts {
                        match part.kind {
                            PartKind::ToolCall(_) => call_part = Some(part.clone()),
                            PartKind::ToolResult(_) => result_part = Some(part.clone()),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let pair = call_part.zip(result_part);
    if pair.is_none() {
        warn(
            warnings,
            call.record,
            target,
            call.position,
            "工具回合缺少完整调用或结果，已跳过该工具上下文",
        );
    }
    pair
}
