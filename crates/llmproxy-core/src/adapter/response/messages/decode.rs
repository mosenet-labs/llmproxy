//! Anthropic 非流式响应消息解码。

use serde_json::Value;

use crate::{
    ir::response::{Message as IrMessage, PartKind, Role, ToolCall},
    protocol::messages::response::message::Message,
};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 顶层 ID、模型、停止原因和用量保留在消息元数据中。
pub fn decode_messages(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let mut raw = wire::object(serde_json::to_value(message)?)?;
            if wire::take_string(&mut raw, "role")? != "assistant" {
                return Err(Error::Invalid("Messages 响应角色必须是 assistant".into()));
            }
            let blocks = raw
                .remove("content")
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| Error::Invalid("Messages 响应 content 必须是数组".into()))?;
            let mut parts = Vec::new();
            for block in blocks {
                let mut block = wire::object(block)?;
                let part = match block.get("type").and_then(Value::as_str) {
                    Some("text") => wire::typed_text(block, PROTOCOL, "text")?,
                    Some("tool_use")
                        if block.get("caller").is_none_or(|caller| {
                            caller.is_null()
                                || caller.get("type").and_then(Value::as_str) == Some("direct")
                        }) =>
                    {
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
                    _ => wire::opaque(PROTOCOL, block),
                };
                parts.push(part);
            }
            Ok(wire::response_message(
                Role::Assistant,
                parts,
                PROTOCOL,
                "parts",
                raw,
            ))
        })
        .collect()
}
