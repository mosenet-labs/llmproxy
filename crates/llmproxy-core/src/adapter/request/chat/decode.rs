//! Chat 原始消息序列解码。

use serde_json::{Map, Value, json};

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role, ToolCall as IrToolCall, ToolResult},
    protocol::chat::request::message::Message,
};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 按原数组顺序解码；Chat 工具结果转成独立的 IR 工具消息。
pub fn decode_chat(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let mut raw = wire::object(serde_json::to_value(message)?)?;
            let role = match wire::take_string(&mut raw, "role")?.as_str() {
                "system" => Role::System,
                "developer" => Role::Developer,
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" | "function" => Role::Tool,
                other => return Err(Error::Invalid(format!("未知 Chat 角色 {other}"))),
            };
            if role == Role::Tool {
                let id = raw
                    .remove("tool_call_id")
                    .and_then(|v| v.as_str().map(str::to_owned));
                let name = raw
                    .remove("name")
                    .and_then(|v| v.as_str().map(str::to_owned));
                let content = raw.remove("content").unwrap_or(Value::Null);
                let form = if id.is_some() { "tool" } else { "function" };
                return Ok(wire::message(
                    role,
                    vec![wire::part(
                        PartKind::ToolResult(ToolResult { id, name, content }),
                        PROTOCOL,
                        "tool_result",
                        Map::new(),
                    )],
                    PROTOCOL,
                    form,
                    raw,
                ));
            }
            let content = raw.remove("content");
            let form = match &content {
                Some(Value::String(_)) => "text",
                Some(Value::Array(_)) => "parts",
                Some(Value::Null) => {
                    raw.insert("content".into(), Value::Null);
                    "null"
                }
                None => "missing",
                _ => return Err(Error::Invalid("Chat content 形状错误".into())),
            };
            let mut parts = Vec::new();
            match content {
                Some(Value::String(text)) => {
                    parts.push(wire::text_part(text, PROTOCOL, "scalar", Map::new()))
                }
                Some(Value::Array(blocks)) => {
                    for block in blocks {
                        let block = wire::object(block)?;
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => parts.push(wire::typed_text(block, PROTOCOL, "text")?),
                            Some("refusal") => parts.push(wire::typed_refusal(block, PROTOCOL)?),
                            _ => parts.push(wire::opaque(PROTOCOL, block)),
                        }
                    }
                }
                _ => {}
            }
            if let Some(Value::Array(calls)) = raw.remove("tool_calls") {
                if calls.is_empty() {
                    raw.insert("tool_calls".into(), json!([]));
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
                        PartKind::ToolCall(IrToolCall {
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
            Ok(wire::message(role, parts, PROTOCOL, form, raw))
        })
        .collect()
}
