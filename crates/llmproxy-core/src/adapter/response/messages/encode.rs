//! 响应 IR 编码为 Anthropic 顶层 Message。

use serde_json::{Value, json};

use crate::{
    ir::response::{Message as IrMessage, PartKind, Role},
    protocol::messages::response::message::Message,
};

use super::super::{Error, Result, reject_unmapped_chat, reject_unmapped_parts, wire};
use super::PROTOCOL;

/// 必须由来源元数据提供 Anthropic 顶层必需的 ID、模型和用量。
pub fn encode_messages(messages: &[IrMessage]) -> Result<Vec<Message>> {
    messages.iter().map(|message| {
        reject_unmapped_chat(message, PROTOCOL)?;
        reject_unmapped_parts(message, PROTOCOL)?;
        if message.role != Role::Assistant { return Err(wire::unsupported_role(message.role)); }
        let mut raw = wire::extra(&message.metadata, PROTOCOL);
        if !["id", "model", "type", "usage"].iter().all(|key| raw.contains_key(*key)) {
            return Err(Error::Unsupported("缺少 Messages 响应的 ID、模型、类型或用量".into()));
        }
        let mut blocks = Vec::new();
        for part in &message.parts {
            let block = match &part.kind {
                PartKind::Text(text) => wire::encode_block(part, PROTOCOL, json!({"type":"text","text":text}))?,
                PartKind::ToolCall(call) => {
                    let id = call.id.as_ref().ok_or_else(|| Error::Unsupported("Messages 工具调用缺少 ID".into()))?;
                    if !call.arguments.is_object() { return Err(Error::Unsupported("Messages 工具参数必须是 JSON 对象".into())); }
                    wire::encode_block(part, PROTOCOL, json!({"type":"tool_use","id":id,"name":call.name,"input":call.arguments}))?
                }
                PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => opaque.data.clone(),
                _ => return Err(Error::Unsupported("Messages 响应消息不支持此内容块".into())),
            };
            blocks.push(block);
        }
        raw.insert("role".into(), json!("assistant"));
        raw.insert("content".into(), Value::Array(blocks));
        Ok(serde_json::from_value(Value::Object(raw))?)
    }).collect()
}
