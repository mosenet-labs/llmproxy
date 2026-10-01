//! IR 消息序列编码为 Chat。

use serde_json::{Map, Value, json};

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::chat::request::message::Message,
};

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 将 IR 序列写成 Chat 消息；保留原 Chat 的可选字段形状。
pub fn encode_chat(messages: &[IrMessage]) -> Result<Vec<Message>> {
    let mut expanded = Vec::new();
    for message in messages {
        if message.role != Role::User
            || !message
                .parts
                .iter()
                .any(|part| matches!(part.kind, PartKind::ToolResult(_)))
        {
            expanded.push(message.clone());
            continue;
        }
        let mut text_parts = Vec::new();
        for part in &message.parts {
            if matches!(part.kind, PartKind::ToolResult(_)) {
                if !text_parts.is_empty() {
                    expanded.push(IrMessage {
                        role: Role::User,
                        parts: std::mem::take(&mut text_parts),
                        metadata: message.metadata.clone(),
                    });
                }
                expanded.push(IrMessage {
                    role: Role::Tool,
                    parts: vec![part.clone()],
                    metadata: Map::new(),
                });
            } else {
                text_parts.push(part.clone());
            }
        }
        if !text_parts.is_empty() {
            expanded.push(IrMessage {
                role: Role::User,
                parts: text_parts,
                metadata: message.metadata.clone(),
            });
        }
    }
    expanded
        .iter()
        .map(|message| {
            let mut raw = wire::extra(&message.metadata, PROTOCOL);
            let role = match message.role {
                Role::System => "system",
                Role::Developer => "developer",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => {
                    if wire::form(&message.metadata, PROTOCOL) == Some("function") {
                        "function"
                    } else {
                        "tool"
                    }
                }
                role => return Err(wire::unsupported_role(role)),
            };
            raw.insert("role".into(), json!(role));
            if message.role == Role::Tool {
                let [part] = message.parts.as_slice() else {
                    return Err(Error::Unsupported("Chat 工具消息只能包含一个结果".into()));
                };
                let PartKind::ToolResult(result) = &part.kind else {
                    return Err(Error::Unsupported("Chat 工具消息需要工具结果".into()));
                };
                if role == "function" {
                    raw.insert(
                        "name".into(),
                        json!(
                            result
                                .name
                                .as_ref()
                                .ok_or_else(|| Error::Unsupported("旧版函数结果缺少名称".into()))?
                        ),
                    );
                } else {
                    raw.insert(
                        "tool_call_id".into(),
                        json!(result.id.as_ref().ok_or_else(|| Error::Unsupported(
                            "Chat 工具结果缺少调用 ID".into()
                        ))?),
                    );
                }
                let content = if result.content.is_object()
                    && wire::form(&part.metadata, PROTOCOL).is_none()
                {
                    json!(serde_json::to_string(&result.content)?)
                } else {
                    result.content.clone()
                };
                raw.insert("content".into(), content);
                return Ok(serde_json::from_value(Value::Object(raw))?);
            }
            let mut blocks = Vec::new();
            let mut calls = Vec::new();
            let mut seen_call = false;
            for part in &message.parts {
                match &part.kind {
                    PartKind::Text(text) => {
                        if seen_call {
                            return Err(Error::Unsupported(
                                "Chat 无法保持工具调用后文本的顺序".into(),
                            ));
                        }
                        blocks.push(wire::encode_block(
                            part,
                            PROTOCOL,
                            json!({"type":"text","text":text}),
                        )?);
                    }
                    PartKind::Refusal(text) if role == "assistant" => {
                        blocks.push(wire::encode_block(
                            part,
                            PROTOCOL,
                            json!({"type":"refusal","refusal":text}),
                        )?);
                    }
                    PartKind::ToolCall(call) if role == "assistant" => {
                        seen_call = true;
                        let id = call
                            .id
                            .as_ref()
                            .ok_or_else(|| Error::Unsupported("Chat 函数调用缺少 ID".into()))?;
                        let args = match &call.arguments {
                            Value::String(s) => s.clone(),
                            value => serde_json::to_string(value)?,
                        };
                        let mut original = wire::extra(&part.metadata, PROTOCOL);
                        let nested = original
                            .remove("function")
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();
                        let function = wire::merge(
                            nested,
                            wire::object(json!({"name":call.name,"arguments":args}))?,
                        );
                        let normalized =
                            wire::object(json!({"id":id,"type":"function","function":function}))?;
                        calls.push(Value::Object(wire::merge(original, normalized)));
                    }
                    PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                        if opaque.data.get("type").and_then(Value::as_str) == Some("custom")
                            && role == "assistant"
                        {
                            seen_call = true;
                            calls.push(opaque.data.clone());
                        } else {
                            blocks.push(opaque.data.clone());
                        }
                    }
                    _ => {
                        return Err(Error::Unsupported(format!(
                            "Chat {role} 消息不支持此内容块"
                        )));
                    }
                }
            }
            if !blocks.is_empty() {
                if blocks.len() == 1
                    && blocks[0].get("type").and_then(Value::as_str) == Some("text")
                    && wire::form(&message.metadata, PROTOCOL) == Some("text")
                {
                    raw.insert("content".into(), blocks[0]["text"].clone());
                } else {
                    raw.insert("content".into(), Value::Array(blocks));
                }
            } else if !raw.contains_key("content")
                && (role != "assistant" || wire::form(&message.metadata, PROTOCOL) == Some("parts"))
            {
                let content = if wire::form(&message.metadata, PROTOCOL) == Some("parts") {
                    json!([])
                } else {
                    json!("")
                };
                raw.insert("content".into(), content);
            }
            if !calls.is_empty() {
                raw.insert("tool_calls".into(), Value::Array(calls));
            }
            Ok(serde_json::from_value(Value::Object(raw))?)
        })
        .collect()
}
