//! IR 消息序列编码为 responses。

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::responses::request::message::Message,
};
use serde_json::{Map, Value, json};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 仅写回消息项；工具调用应由未来的 RequestItem 适配器处理。
pub fn encode_responses(messages: &[IrMessage]) -> Result<Vec<Message>> {
    messages
        .iter()
        .map(|message| {
            wire::reject_unmapped_source(message, PROTOCOL)?;
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Developer => "developer",
                role => return Err(wire::unsupported_role(role)),
            };
            let mut raw = wire::extra(&message.metadata, PROTOCOL);
            let form = wire::form(&message.metadata, PROTOCOL).unwrap_or("");
            let output = form.starts_with("output_");
            let mut blocks = Vec::new();
            for part in &message.parts {
                let block = match &part.kind {
                    PartKind::Text(text) => {
                        let kind = if output { "output_text" } else { "input_text" };
                        let mut block = Map::new();
                        block.insert("type".into(), json!(kind));
                        block.insert("text".into(), json!(text));
                        let mut block = wire::encode_block(part, PROTOCOL, Value::Object(block))?;
                        if output {
                            block
                                .as_object_mut()
                                .unwrap()
                                .entry("annotations")
                                .or_insert(json!([]));
                        }
                        block
                    }
                    PartKind::Refusal(text) if output => wire::encode_block(
                        part,
                        PROTOCOL,
                        json!({"type":"refusal","refusal":text}),
                    )?,
                    PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => opaque.data.clone(),
                    _ => {
                        return Err(Error::Unsupported(
                            "Responses 消息不支持工具项或此内容块".into(),
                        ));
                    }
                };
                blocks.push(block);
            }
            let content = if blocks.len() == 1
                && form.ends_with("_text")
                && matches!(message.parts[0].kind, PartKind::Text(_))
            {
                blocks[0]["text"].clone()
            } else {
                Value::Array(blocks)
            };
            raw.insert("role".into(), json!(role));
            raw.insert("content".into(), content);
            Ok(serde_json::from_value(Value::Object(raw))?)
        })
        .collect()
}
