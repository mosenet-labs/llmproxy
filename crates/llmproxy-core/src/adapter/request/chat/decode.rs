//! 从 Chat 消息类型直接提取 IR，不序列化整条消息。
use super::super::{Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, Part, PartKind, Role, ToolCall as IrCall, ToolResult},
    protocol::{OptionalNullable as O, chat::request::message::*},
};
use serde_json::{Map, Value};

/// Chat 角色、正文和工具调用均通过 enum 分支读取。
pub fn decode_chat(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let (role, parts, form, extra) = match message {
                Message::System(m) => {
                    let (form, parts) = content(&m.content)?;
                    let mut extra = m.extra.clone();
                    wire::put_option(&mut extra, "name", &m.name)?;
                    (Role::System, parts, form, extra)
                }
                Message::Developer(m) => {
                    let (form, parts) = content(&m.content)?;
                    let mut extra = m.extra.clone();
                    wire::put_option(&mut extra, "name", &m.name)?;
                    (Role::Developer, parts, form, extra)
                }
                Message::User(m) => {
                    let (form, parts) = content(&m.content)?;
                    let mut extra = m.extra.clone();
                    wire::put_option(&mut extra, "name", &m.name)?;
                    (Role::User, parts, form, extra)
                }
                Message::Assistant {
                    audio,
                    content: body,
                    function_call,
                    name,
                    refusal,
                    tool_calls,
                    extra,
                } => {
                    let mut extra = extra.clone();
                    wire::put(&mut extra, "audio", audio)?;
                    wire::put(&mut extra, "function_call", function_call)?;
                    wire::put_option(&mut extra, "name", name)?;
                    wire::put(&mut extra, "refusal", refusal)?;
                    let (form, mut parts) = match body {
                        O::Value(body) => content(body)?,
                        O::Null => {
                            extra.insert("content".into(), Value::Null);
                            ("null", vec![])
                        }
                        O::Missing => ("missing", vec![]),
                    };
                    if let Some(calls) = tool_calls {
                        if calls.is_empty() {
                            extra.insert("tool_calls".into(), Value::Array(vec![]));
                        }
                        for call in calls {
                            parts.push(decode_call(call)?);
                        }
                    }
                    (Role::Assistant, parts, form, extra)
                }
                Message::Tool {
                    content,
                    tool_call_id,
                    extra,
                } => {
                    let content = serde_json::to_value(content)?;
                    (
                        Role::Tool,
                        vec![wire::part(
                            PartKind::ToolResult(ToolResult {
                                id: Some(tool_call_id.clone()),
                                name: None,
                                content,
                            }),
                            PROTOCOL,
                            "tool_result",
                            Map::new(),
                        )],
                        "tool",
                        extra.clone(),
                    )
                }
                Message::Function {
                    content,
                    name,
                    extra,
                } => (
                    Role::Tool,
                    vec![wire::part(
                        PartKind::ToolResult(ToolResult {
                            id: None,
                            name: Some(name.clone()),
                            content: content
                                .as_ref()
                                .map(|s| Value::String(s.clone()))
                                .unwrap_or(Value::Null),
                        }),
                        PROTOCOL,
                        "tool_result",
                        Map::new(),
                    )],
                    "function",
                    extra.clone(),
                ),
            };
            Ok(wire::message(role, parts, PROTOCOL, form, extra))
        })
        .collect()
}
/// 函数参数允许保留尚未解析成功的字符串，以便同协议往返。
pub(in crate::adapter) fn decode_call(call: &ToolCall) -> Result<Part> {
    match call {
        ToolCall::Function {
            id,
            function,
            extra,
        } => {
            let mut extra = extra.clone();
            if !function.extra.is_empty() {
                extra.insert("function".into(), Value::Object(function.extra.clone()));
            }
            Ok(wire::part(
                PartKind::ToolCall(IrCall {
                    id: Some(id.clone()),
                    name: function.name.clone(),
                    arguments: serde_json::from_str(&function.arguments)
                        .unwrap_or_else(|_| Value::String(function.arguments.clone())),
                }),
                PROTOCOL,
                "function",
                extra,
            ))
        }
        _ => wire::opaque_value(PROTOCOL, call),
    }
}
/// 保留字符串与内容块数组的形状。
fn content<P: DecodePart>(content: &Content<P>) -> Result<(&'static str, Vec<Part>)> {
    match content {
        Content::Text(text) => Ok((
            "text",
            vec![wire::text_part(
                text.clone(),
                PROTOCOL,
                "scalar",
                Map::new(),
            )],
        )),
        Content::Parts(parts) => Ok((
            "parts",
            parts
                .iter()
                .map(DecodePart::decode)
                .collect::<Result<_>>()?,
        )),
    }
}
/// 三种 Chat 文本内容块共用直接字段投影。
trait DecodePart {
    fn decode(&self) -> Result<Part>;
}
impl DecodePart for TextPart {
    fn decode(&self) -> Result<Part> {
        let Self::Text {
            text,
            prompt_cache_breakpoint,
            extra,
        } = self;
        let mut extra = extra.clone();
        wire::put(
            &mut extra,
            "prompt_cache_breakpoint",
            prompt_cache_breakpoint,
        )?;
        Ok(wire::text_part(text.clone(), PROTOCOL, "text", extra))
    }
}
impl DecodePart for UserPart {
    fn decode(&self) -> Result<Part> {
        match self {
            Self::Text {
                text,
                prompt_cache_breakpoint,
                extra,
            } => {
                let mut extra = extra.clone();
                wire::put(
                    &mut extra,
                    "prompt_cache_breakpoint",
                    prompt_cache_breakpoint,
                )?;
                Ok(wire::text_part(text.clone(), PROTOCOL, "text", extra))
            }
            _ => wire::opaque_value(PROTOCOL, self),
        }
    }
}
impl DecodePart for AssistantPart {
    fn decode(&self) -> Result<Part> {
        match self {
            Self::Text {
                text,
                prompt_cache_breakpoint,
                extra,
            } => {
                let mut extra = extra.clone();
                wire::put(
                    &mut extra,
                    "prompt_cache_breakpoint",
                    prompt_cache_breakpoint,
                )?;
                Ok(wire::text_part(text.clone(), PROTOCOL, "text", extra))
            }
            Self::Refusal { refusal, extra } => Ok(wire::part(
                PartKind::Refusal(refusal.clone()),
                PROTOCOL,
                "refusal",
                extra.clone(),
            )),
        }
    }
}
