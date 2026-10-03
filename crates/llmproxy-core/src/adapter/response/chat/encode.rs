//! 响应 IR 直接构造 Chat 消息类型。
use super::super::{Error, Result, reject_unmapped_parts, wire};
use super::PROTOCOL;
use crate::{
    ir::response::{Message as IrMessage, PartKind, Role},
    protocol::{
        OptionalNullable as O,
        chat::response::message::{AssistantRole, Message},
    },
};
use serde_json::Value;
/// 保留来源可选字段形状，并直接更新正文、拒绝和工具字段。
pub fn encode_chat(messages: &[IrMessage]) -> Result<Vec<Message>> {
    messages
        .iter()
        .map(|message| {
            reject_unmapped_parts(message, PROTOCOL)?;
            if message.role != Role::Assistant {
                return Err(wire::unsupported_role(message.role));
            }
            let mut extra = wire::extra(&message.metadata, PROTOCOL);
            let mut content = wire::take(&mut extra, "content")?;
            let mut refusal = wire::take(&mut extra, "refusal")?;
            let original_calls = wire::take(&mut extra, "tool_calls")?;
            let mut calls = Vec::new();
            let mut seen_text = false;
            let mut seen_refusal = false;
            for part in &message.parts {
                match &part.kind {
                    PartKind::Text(text) if !seen_text => {
                        seen_text = true;
                        content = O::Value(text.clone());
                    }
                    PartKind::Refusal(text) if !seen_refusal => {
                        seen_refusal = true;
                        refusal = O::Value(text.clone());
                    }
                    PartKind::ToolCall(_) => {
                        calls.push(crate::adapter::request::chat::encode_call(part)?)
                    }
                    PartKind::Opaque(opaque)
                        if opaque.protocol == PROTOCOL
                            && opaque.data.get("type").and_then(Value::as_str)
                                == Some("custom") =>
                    {
                        calls.push(serde_json::from_value(opaque.data.clone())?)
                    }
                    _ => {
                        return Err(Error::Unsupported(
                            "Chat 响应消息无法表示此内容或顺序".into(),
                        ));
                    }
                }
            }
            if content.is_missing() && wire::form(&message.metadata, PROTOCOL).is_none() {
                content = O::Null;
            }
            Ok(Message {
                role: AssistantRole::Assistant,
                content,
                refusal,
                tool_calls: if calls.is_empty() {
                    original_calls
                } else {
                    O::Value(calls)
                },
                audio: wire::take(&mut extra, "audio")?,
                annotations: wire::take(&mut extra, "annotations")?,
                function_call: wire::take(&mut extra, "function_call")?,
                extra,
            })
        })
        .collect()
}
