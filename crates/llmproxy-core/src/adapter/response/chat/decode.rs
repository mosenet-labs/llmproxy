//! Chat 非流式输出消息解码。

use serde_json::{Map, Value};

use crate::{
    ir::response::{Message as IrMessage, PartKind, Role, ToolCall},
    protocol::chat::response::message::Message,
};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 保留正文、拒绝、工具调用及原协议的 `null` 状态。
pub fn decode_chat(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let mut raw = wire::object(serde_json::to_value(message)?)?;
            if wire::take_string(&mut raw, "role")? != "assistant" {
                return Err(Error::Invalid("Chat 响应角色必须是 assistant".into()));
            }
            let content = raw.remove("content");
            let form = match &content {
                Some(Value::String(_)) => "text",
                Some(Value::Null) => {
                    raw.insert("content".into(), Value::Null);
                    "null"
                }
                None => "missing",
                _ => return Err(Error::Invalid("Chat 响应 content 必须是文本或 null".into())),
            };
            let mut parts = Vec::new();
            if let Some(Value::String(text)) = content {
                parts.push(wire::text_part(text, PROTOCOL, "content", Map::new()));
            }
            match raw.remove("refusal") {
                Some(Value::String(text)) => parts.push(wire::part(
                    PartKind::Refusal(text),
                    PROTOCOL,
                    "refusal",
                    Map::new(),
                )),
                Some(Value::Null) => {
                    raw.insert("refusal".into(), Value::Null);
                }
                Some(_) => return Err(Error::Invalid("Chat refusal 必须是文本或 null".into())),
                None => {}
            }
            match raw.remove("tool_calls") {
                Some(Value::Array(calls)) => {
                    if calls.is_empty() {
                        raw.insert("tool_calls".into(), Value::Array(Vec::new()));
                    }
                    for call in calls {
                        let mut call = wire::object(call)?;
                        if call.get("type").and_then(Value::as_str) != Some("function") {
                            parts.push(wire::opaque(PROTOCOL, call));
                            continue;
                        }
                        call.remove("type");
                        let id = wire::take_string(&mut call, "id")?;
                        let mut function = wire::object(
                            call.remove("function")
                                .ok_or_else(|| Error::Invalid("缺少 function".into()))?,
                        )?;
                        let name = wire::take_string(&mut function, "name")?;
                        let arguments = wire::take_string(&mut function, "arguments")?;
                        if !function.is_empty() {
                            call.insert("function".into(), Value::Object(function));
                        }
                        let arguments =
                            serde_json::from_str(&arguments).unwrap_or(Value::String(arguments));
                        parts.push(wire::part(
                            PartKind::ToolCall(ToolCall {
                                id: Some(id),
                                name,
                                arguments,
                            }),
                            PROTOCOL,
                            "function",
                            call,
                        ));
                    }
                }
                Some(Value::Null) => {
                    raw.insert("tool_calls".into(), Value::Null);
                }
                Some(_) => return Err(Error::Invalid("Chat tool_calls 必须是数组或 null".into())),
                None => {}
            }
            Ok(wire::response_message(
                Role::Assistant,
                parts,
                PROTOCOL,
                form,
                raw,
            ))
        })
        .collect()
}
