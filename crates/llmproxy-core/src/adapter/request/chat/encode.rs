//! IR 消息序列编码为 Chat。

use crate::ir::request::Part;
use crate::protocol::{
    OptionalNullable as O,
    chat::request::message::{
        AssistantPart, Content, ContentMessage, FunctionCall, TextPart, ToolCall, UserPart,
    },
};
use serde_json::{Map, Value};

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::chat::request::message::Message,
};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 将 IR 序列写成 Chat 消息；保留原 Chat 的可选字段形状。
pub fn encode_chat(messages: &[IrMessage]) -> Result<Vec<Message>> {
    let mut expanded = Vec::new();
    for message in messages {
        if message.role != Role::User
            || !message
                .parts
                .iter()
                .any(|part| matches!(part.kind, PartKind::ToolResult(_)))
        {
            expanded.push(message.clone());
            continue;
        }
        let mut text_parts = Vec::new();
        for part in &message.parts {
            if matches!(part.kind, PartKind::ToolResult(_)) {
                if !text_parts.is_empty() {
                    expanded.push(IrMessage {
                        role: Role::User,
                        parts: std::mem::take(&mut text_parts),
                        metadata: message.metadata.clone(),
                    });
                }
                expanded.push(IrMessage {
                    role: Role::Tool,
                    parts: vec![part.clone()],
                    metadata: Map::new(),
                });
            } else {
                text_parts.push(part.clone());
            }
        }
        if !text_parts.is_empty() {
            expanded.push(IrMessage {
                role: Role::User,
                parts: text_parts,
                metadata: message.metadata.clone(),
            });
        }
    }
    expanded.iter().map(encode_message).collect()
}

/// 直接构造 Chat 的角色变体，未规范化叶子字段从元数据恢复。
fn encode_message(message: &IrMessage) -> Result<Message> {
    let mut extra = wire::extra(&message.metadata, PROTOCOL);
    let form = wire::form(&message.metadata, PROTOCOL);
    if message.role == Role::Tool {
        let [part] = message.parts.as_slice() else {
            return Err(Error::Unsupported("Chat 工具消息只能包含一个结果".into()));
        };
        let PartKind::ToolResult(result) = &part.kind else {
            return Err(Error::Unsupported("Chat 工具消息需要工具结果".into()));
        };
        let content =
            if result.content.is_object() && wire::form(&part.metadata, PROTOCOL).is_none() {
                Value::String(serde_json::to_string(&result.content)?)
            } else {
                result.content.clone()
            };
        return if form == Some("function") {
            Ok(Message::Function {
                name: result
                    .name
                    .clone()
                    .ok_or_else(|| Error::Unsupported("旧版函数结果缺少名称".into()))?,
                content: serde_json::from_value(content)?,
                extra,
            })
        } else {
            Ok(Message::Tool {
                tool_call_id: result
                    .id
                    .clone()
                    .ok_or_else(|| Error::Unsupported("Chat 工具结果缺少调用 ID".into()))?,
                content: serde_json::from_value(content)?,
                extra,
            })
        };
    }
    let mut parts = Vec::new();
    let mut calls = Vec::new();
    let mut seen_call = false;
    for part in &message.parts {
        match &part.kind {
            PartKind::ToolCall(_) if message.role == Role::Assistant => {
                seen_call = true;
                calls.push(encode_call(part)?);
            }
            PartKind::Opaque(opaque)
                if opaque.protocol == PROTOCOL
                    && opaque.data.get("type").and_then(Value::as_str) == Some("custom")
                    && message.role == Role::Assistant =>
            {
                seen_call = true;
                calls.push(serde_json::from_value(opaque.data.clone())?);
            }
            _ => {
                if seen_call && matches!(part.kind, PartKind::Text(_)) {
                    return Err(Error::Unsupported(
                        "Chat 无法保持工具调用后文本的顺序".into(),
                    ));
                }
                parts.push(part);
            }
        }
    }
    let name = wire::take_option(&mut extra, "name")?;
    Ok(match message.role {
        Role::System => Message::System(ContentMessage {
            content: content(&parts, form)?,
            name,
            extra,
        }),
        Role::Developer => Message::Developer(ContentMessage {
            content: content(&parts, form)?,
            name,
            extra,
        }),
        Role::User => Message::User(ContentMessage {
            content: content(&parts, form)?,
            name,
            extra,
        }),
        Role::Assistant => {
            let original_content = wire::take(&mut extra, "content")?;
            let original_calls = wire::take_option(&mut extra, "tool_calls")?;
            let content = if !parts.is_empty() || form == Some("parts") {
                O::Value(content(&parts, form)?)
            } else {
                original_content
            };
            let tool_calls = if calls.is_empty() {
                original_calls
            } else {
                Some(calls)
            };
            Message::Assistant {
                audio: wire::take(&mut extra, "audio")?,
                content,
                function_call: wire::take(&mut extra, "function_call")?,
                name,
                refusal: wire::take(&mut extra, "refusal")?,
                tool_calls,
                extra,
            }
        }
        role => return Err(wire::unsupported_role(role)),
    })
}
/// 从 IR 调用直接构造函数调用类型。
pub(in crate::adapter) fn encode_call(part: &Part) -> Result<ToolCall> {
    let PartKind::ToolCall(call) = &part.kind else {
        return Err(Error::Unsupported("需要函数调用".into()));
    };
    let id = call
        .id
        .clone()
        .ok_or_else(|| Error::Unsupported("Chat 函数调用缺少 ID".into()))?;
    let arguments = match &call.arguments {
        Value::String(s) => s.clone(),
        value => serde_json::to_string(value)?,
    };
    let mut extra = wire::extra(&part.metadata, PROTOCOL);
    let function_extra = extra
        .remove("function")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    Ok(ToolCall::Function {
        id,
        function: FunctionCall {
            name: call.name.clone(),
            arguments,
            extra: function_extra,
        },
        extra,
    })
}
/// 标量文本保留标量形态，块数组逐个构造对应类型。
fn content<P: EncodePart>(parts: &[&Part], form: Option<&str>) -> Result<Content<P>> {
    if form == Some("text")
        && let [
            Part {
                kind: PartKind::Text(text),
                ..
            },
        ] = parts
    {
        return Ok(Content::Text(text.clone()));
    }
    if parts.is_empty() && form != Some("parts") {
        return Ok(Content::Text(String::new()));
    }
    Ok(Content::Parts(
        parts
            .iter()
            .map(|part| P::encode(part))
            .collect::<Result<_>>()?,
    ))
}
/// Chat 三种内容块的公共编码契约。
trait EncodePart: serde::de::DeserializeOwned {
    fn encode(part: &Part) -> Result<Self>;
}
impl EncodePart for TextPart {
    fn encode(part: &Part) -> Result<Self> {
        let mut extra = wire::extra(&part.metadata, PROTOCOL);
        match &part.kind {
            PartKind::Text(text) => Ok(Self::Text {
                text: text.clone(),
                prompt_cache_breakpoint: wire::take(&mut extra, "prompt_cache_breakpoint")?,
                extra,
            }),
            PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                Ok(serde_json::from_value(opaque.data.clone())?)
            }
            _ => Err(Error::Unsupported("Chat 内容块无法转换".into())),
        }
    }
}
impl EncodePart for UserPart {
    fn encode(part: &Part) -> Result<Self> {
        let mut extra = wire::extra(&part.metadata, PROTOCOL);
        match &part.kind {
            PartKind::Text(text) => Ok(Self::Text {
                text: text.clone(),
                prompt_cache_breakpoint: wire::take(&mut extra, "prompt_cache_breakpoint")?,
                extra,
            }),
            PartKind::Media(media) => match crate::adapter::media::encode(media, PROTOCOL)? {
                crate::ir::media::OriginalMedia::Chat(part) => Ok(part),
                _ => unreachable!(),
            },
            PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                Ok(serde_json::from_value(opaque.data.clone())?)
            }
            _ => Err(Error::Unsupported("Chat 内容块无法转换".into())),
        }
    }
}
impl EncodePart for AssistantPart {
    fn encode(part: &Part) -> Result<Self> {
        let mut extra = wire::extra(&part.metadata, PROTOCOL);
        match &part.kind {
            PartKind::Text(text) => Ok(Self::Text {
                text: text.clone(),
                prompt_cache_breakpoint: wire::take(&mut extra, "prompt_cache_breakpoint")?,
                extra,
            }),
            PartKind::Refusal(text) => Ok(Self::Refusal {
                refusal: text.clone(),
                extra,
            }),
            PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                Ok(serde_json::from_value(opaque.data.clone())?)
            }
            _ => Err(Error::Unsupported("Chat 内容块无法转换".into())),
        }
    }
}
