//! gemini 原始消息序列解码。

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role, ToolCall, ToolResult},
    protocol::gemini::request::message::Message,
};
use serde_json::{Value, json};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 省略的 `role` 原样映射为 IR 未指定角色。
pub fn decode_gemini(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let mut raw = wire::object(serde_json::to_value(message)?)?;
            let role = match raw.remove("role").as_ref().and_then(Value::as_str) {
                Some("user") => Role::User,
                Some("model") => Role::Assistant,
                None => Role::Unspecified,
                Some(other) => return Err(Error::Invalid(format!("未知 Gemini 角色 {other}"))),
            };
            let blocks = raw
                .remove("parts")
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| Error::Invalid("Gemini parts 必须是数组".into()))?;
            let mut parts = Vec::new();
            for block in blocks {
                let mut block = wire::object(block)?;
                let main_fields = [
                    "text",
                    "functionCall",
                    "functionResponse",
                    "inlineData",
                    "fileData",
                    "toolCall",
                    "toolResponse",
                    "executableCode",
                    "codeExecutionResult",
                ];
                let count = main_fields
                    .iter()
                    .filter(|key| block.contains_key(**key))
                    .count();
                let part = if count == 1 && block.get("text").and_then(Value::as_str).is_some() {
                    wire::text_part(
                        wire::take_string(&mut block, "text")?,
                        PROTOCOL,
                        "text",
                        block,
                    )
                } else if count == 1 && block.contains_key("functionCall") {
                    let mut call = wire::object(block.remove("functionCall").unwrap())?;
                    let name = wire::take_string(&mut call, "name")?;
                    let id_value = call.remove("id");
                    let id = id_value.as_ref().and_then(Value::as_str).map(str::to_owned);
                    if id_value == Some(Value::Null) {
                        call.insert("id".into(), Value::Null);
                    }
                    let args = call.remove("args");
                    if args == Some(Value::Null) {
                        call.insert("args".into(), Value::Null);
                    }
                    let form = if args.is_some() {
                        "function_call"
                    } else {
                        "function_call_no_args"
                    };
                    let mut residual = block;
                    if !call.is_empty() {
                        residual.insert("functionCall".into(), Value::Object(call));
                    }
                    wire::part(
                        PartKind::ToolCall(ToolCall {
                            id,
                            name,
                            arguments: args.unwrap_or(json!({})),
                        }),
                        PROTOCOL,
                        form,
                        residual,
                    )
                } else if count == 1 && block.contains_key("functionResponse") {
                    let mut result = wire::object(block.remove("functionResponse").unwrap())?;
                    let name = wire::take_string(&mut result, "name")?;
                    let id_value = result.remove("id");
                    let id = id_value.as_ref().and_then(Value::as_str).map(str::to_owned);
                    if id_value == Some(Value::Null) {
                        result.insert("id".into(), Value::Null);
                    }
                    let content = result
                        .remove("response")
                        .ok_or_else(|| Error::Invalid("functionResponse 缺少 response".into()))?;
                    let mut residual = block;
                    if !result.is_empty() {
                        residual.insert("functionResponse".into(), Value::Object(result));
                    }
                    wire::part(
                        PartKind::ToolResult(ToolResult {
                            id,
                            name: Some(name),
                            content,
                        }),
                        PROTOCOL,
                        "function_response",
                        residual,
                    )
                } else {
                    wire::opaque(PROTOCOL, block)
                };
                parts.push(part);
            }
            Ok(wire::message(role, parts, PROTOCOL, "parts", raw))
        })
        .collect()
}
