//! responses 原始消息序列解码。

use crate::{
    ir::request::{Message as IrMessage, Role},
    protocol::responses::request::message::Message,
};
use serde_json::{Map, Value};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 解码现有 DTO 可表达的 Easy、Input 和 Output 消息。
pub fn decode_responses(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let form = match message {
                Message::Easy(_) => "easy",
                Message::Input(_) => "input",
                Message::Output(_) => "output",
            };
            let mut raw = wire::object(serde_json::to_value(message)?)?;
            let role = match wire::take_string(&mut raw, "role")?.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" => Role::System,
                "developer" => Role::Developer,
                other => return Err(Error::Invalid(format!("未知 Responses 角色 {other}"))),
            };
            let content = raw
                .remove("content")
                .ok_or_else(|| Error::Invalid("缺少 content".into()))?;
            let (shape, parts) = match content {
                Value::String(text) => (
                    "text",
                    vec![wire::text_part(text, PROTOCOL, "scalar", Map::new())],
                ),
                Value::Array(blocks) => {
                    let mut parts = Vec::new();
                    for block in blocks {
                        let block = wire::object(block)?;
                        let part = match block.get("type").and_then(Value::as_str) {
                            Some("input_text" | "output_text") => {
                                let kind = block
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .unwrap()
                                    .to_owned();
                                wire::typed_text(block, PROTOCOL, &kind)?
                            }
                            Some("refusal") => wire::typed_refusal(block, PROTOCOL)?,
                            _ => wire::opaque(PROTOCOL, block),
                        };
                        parts.push(part);
                    }
                    ("parts", parts)
                }
                _ => return Err(Error::Invalid("Responses content 形状错误".into())),
            };
            Ok(wire::message(
                role,
                parts,
                PROTOCOL,
                &format!("{form}_{shape}"),
                raw,
            ))
        })
        .collect()
}
