//! 响应 IR 编码为 Chat 非流式输出消息。

use serde_json::{Value, json};

use crate::{
    ir::response::{Message as IrMessage, PartKind, Role},
    protocol::chat::response::message::Message,
};

use super::super::{Error, Result, reject_unmapped_parts, wire};
use super::PROTOCOL;

/// 仅生成 Chat 能表示的单段文本、拒绝及普通函数调用。
pub fn encode_chat(messages: &[IrMessage]) -> Result<Vec<Message>> {
    messages
        .iter()
        .map(|message| {
            reject_unmapped_parts(message, PROTOCOL)?;
            if message.role != Role::Assistant {
                return Err(wire::unsupported_role(message.role));
            }
            let mut raw = wire::extra(&message.metadata, PROTOCOL);
            raw.insert("role".into(), json!("assistant"));
            let mut calls = Vec::new();
            let mut seen_text = false;
            let mut seen_refusal = false;
            for part in &message.parts {
                match &part.kind {
                    PartKind::Text(text) if !seen_text => {
                        seen_text = true;
                        raw.insert("content".into(), json!(text));
                    }
                    PartKind::Refusal(text) if !seen_refusal => {
                        seen_refusal = true;
                        raw.insert("refusal".into(), json!(text));
                    }
                    PartKind::ToolCall(call) => {
                        let id = call
                            .id
                            .as_ref()
                            .ok_or_else(|| Error::Unsupported("Chat 函数调用缺少 ID".into()))?;
                        let arguments = match &call.arguments {
                            Value::String(text) => text.clone(),
                            value => serde_json::to_string(value)?,
                        };
                        let mut residual = wire::extra(&part.metadata, PROTOCOL);
                        let nested = residual
                            .remove("function")
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();
                        let function = wire::merge(
                            nested,
                            wire::object(json!({"name":call.name,"arguments":arguments}))?,
                        );
                        calls.push(Value::Object(wire::merge(
                            residual,
                            wire::object(json!({"id":id,"type":"function","function":function}))?,
                        )));
                    }
                    PartKind::Opaque(opaque)
                        if opaque.protocol == PROTOCOL
                            && opaque.data.get("type").and_then(Value::as_str)
                                == Some("custom") =>
                    {
                        calls.push(opaque.data.clone())
                    }
                    _ => {
                        return Err(Error::Unsupported(
                            "Chat 响应消息无法表示此内容或顺序".into(),
                        ));
                    }
                }
            }
            if !calls.is_empty() {
                raw.insert("tool_calls".into(), Value::Array(calls));
            }
            if !raw.contains_key("content") && wire::form(&message.metadata, PROTOCOL).is_none() {
                raw.insert("content".into(), Value::Null);
            }
            Ok(serde_json::from_value(Value::Object(raw))?)
        })
        .collect()
}
