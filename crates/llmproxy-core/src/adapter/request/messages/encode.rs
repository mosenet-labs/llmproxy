//! IR 消息序列编码为 messages。

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::messages::request::message::Message,
};
use serde_json::{Value, json};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 将相邻 Chat 工具结果接到前一条用户消息，保留原有片段顺序。
pub fn encode_messages(messages: &[IrMessage]) -> Result<Vec<Message>> {
    let mut output: Vec<Value> = Vec::new();
    for message in messages {
        wire::reject_unmapped_source(message, PROTOCOL)?;
        let role = match message.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            role => return Err(wire::unsupported_role(role)),
        };
        let mut blocks = Vec::new();
        for part in &message.parts {
            let normalized = match &part.kind {
                PartKind::Text(text) => json!({"type":"text","text":text}),
                PartKind::ToolCall(call) if role == "assistant" => {
                    let id = call
                        .id
                        .as_ref()
                        .ok_or_else(|| Error::Unsupported("Messages 工具调用缺少 ID".into()))?;
                    if !call.arguments.is_object() {
                        return Err(Error::Unsupported(
                            "Messages 工具参数必须是 JSON 对象".into(),
                        ));
                    }
                    json!({"type":"tool_use","id":id,"name":call.name,"input":call.arguments})
                }
                PartKind::ToolResult(result) if role == "user" => {
                    let id = result
                        .id
                        .as_ref()
                        .ok_or_else(|| Error::Unsupported("Messages 工具结果缺少 ID".into()))?;
                    let mut block = json!({"type":"tool_result","tool_use_id":id});
                    if !result.content.is_null() {
                        block["content"] = if result.content.is_object() {
                            json!(serde_json::to_string(&result.content)?)
                        } else {
                            result.content.clone()
                        };
                    }
                    block
                }
                PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => opaque.data.clone(),
                _ => {
                    return Err(Error::Unsupported(format!(
                        "Messages {role} 消息不支持此内容块"
                    )));
                }
            };
            blocks.push(wire::encode_block(part, PROTOCOL, normalized)?);
        }
        let original_form = wire::form(&message.metadata, PROTOCOL);
        let mut content = if blocks.len() == 1
            && blocks[0].get("type").and_then(Value::as_str) == Some("text")
            && original_form == Some("text")
        {
            blocks.remove(0).get("text").cloned().unwrap()
        } else {
            Value::Array(blocks)
        };
        if message.role == Role::Tool
            && original_form.is_none()
            && let Value::Array(blocks) = &mut content
            && wire::append_to_previous_user(&mut output, "content", blocks)
        {
            continue;
        }
        let mut raw = wire::extra(&message.metadata, PROTOCOL);
        raw.insert("role".into(), json!(role));
        raw.insert("content".into(), content);
        output.push(Value::Object(raw));
    }
    output
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(Into::into))
        .collect()
}
