//! messages 原始消息序列解码。

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role, ToolCall, ToolResult},
    protocol::messages::request::message::Message,
};
use serde_json::{Map, Value};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 保持内容块顺序，把标准工具调用和结果规范化。
pub fn decode_messages(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let mut raw = wire::object(serde_json::to_value(message)?)?;
            let role = match wire::take_string(&mut raw, "role")?.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" => Role::System,
                other => return Err(Error::Invalid(format!("未知 Messages 角色 {other}"))),
            };
            let content = raw
                .remove("content")
                .ok_or_else(|| Error::Invalid("缺少 content".into()))?;
            let (form, parts) = match content {
                Value::String(text) => (
                    "text",
                    vec![wire::text_part(text, PROTOCOL, "scalar", Map::new())],
                ),
                Value::Array(blocks) => {
                    let mut parts = Vec::new();
                    for block in blocks {
                        let mut block = wire::object(block)?;
                        let kind = block
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let part = match kind.as_str() {
                            "text" => wire::typed_text(block, PROTOCOL, "text")?,
                            "tool_use" => {
                                block.remove("type");
                                let id = wire::take_string(&mut block, "id")?;
                                let name = wire::take_string(&mut block, "name")?;
                                let arguments = block
                                    .remove("input")
                                    .ok_or_else(|| Error::Invalid("tool_use 缺少 input".into()))?;
                                wire::part(
                                    PartKind::ToolCall(ToolCall {
                                        id: Some(id),
                                        name,
                                        arguments,
                                    }),
                                    PROTOCOL,
                                    "tool_use",
                                    block,
                                )
                            }
                            "tool_result" => {
                                block.remove("type");
                                let id = wire::take_string(&mut block, "tool_use_id")?;
                                let content = block.remove("content").unwrap_or(Value::Null);
                                if content.is_null() {
                                    block.insert("content".into(), Value::Null);
                                }
                                wire::part(
                                    PartKind::ToolResult(ToolResult {
                                        id: Some(id),
                                        name: None,
                                        content,
                                    }),
                                    PROTOCOL,
                                    "tool_result",
                                    block,
                                )
                            }
                            _ => wire::opaque(PROTOCOL, block),
                        };
                        parts.push(part);
                    }
                    ("parts", parts)
                }
                _ => return Err(Error::Invalid("Messages content 形状错误".into())),
            };
            Ok(wire::message(role, parts, PROTOCOL, form, raw))
        })
        .collect()
}
