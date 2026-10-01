//! IR 消息序列编码为 gemini。

use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::gemini::request::message::Message,
};
use serde_json::{Value, json};
use std::collections::HashMap;

use super::super::{Error, Result, wire};
use super::PROTOCOL;

/// 工具调用和结果分别映射为 `functionCall`、`functionResponse`。
pub fn encode_gemini(messages: &[IrMessage]) -> Result<Vec<Message>> {
    let mut output: Vec<Value> = Vec::new();
    let mut call_names = HashMap::new();
    for message in messages {
        wire::reject_unmapped_source(message, PROTOCOL)?;
        let role = match message.role {
            Role::User | Role::Tool => Some("user"),
            Role::Assistant => Some("model"),
            Role::Unspecified if wire::form(&message.metadata, PROTOCOL).is_some() => None,
            role => return Err(wire::unsupported_role(role)),
        };
        let mut blocks = Vec::new();
        for part in &message.parts {
            let block = match &part.kind {
                PartKind::Text(text) => json!({"text":text}),
                PartKind::ToolCall(call) if role == Some("model") => {
                    if !call.arguments.is_object()
                        && !(call.arguments.is_null()
                            && wire::form(&part.metadata, PROTOCOL).is_some())
                    {
                        return Err(Error::Unsupported("Gemini 函数参数必须是 JSON 对象".into()));
                    }
                    let mut function = wire::extra(&part.metadata, PROTOCOL)
                        .remove("functionCall")
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    function.insert("name".into(), json!(call.name));
                    if let Some(id) = &call.id {
                        function.insert("id".into(), json!(id));
                        call_names.insert(id.clone(), call.name.clone());
                    }
                    if wire::form(&part.metadata, PROTOCOL) != Some("function_call_no_args")
                        || call.arguments != json!({})
                    {
                        function.insert("args".into(), call.arguments.clone());
                    }
                    json!({"functionCall":function})
                }
                PartKind::ToolResult(result) if role == Some("user") => {
                    let name = result
                        .name
                        .as_ref()
                        .or_else(|| result.id.as_ref().and_then(|id| call_names.get(id)))
                        .ok_or_else(|| Error::Unsupported("Gemini 函数结果缺少名称".into()))?;
                    let mut function = wire::extra(&part.metadata, PROTOCOL)
                        .remove("functionResponse")
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                    function.insert("name".into(), json!(name));
                    if let Some(id) = &result.id {
                        function.insert("id".into(), json!(id));
                    }
                    let response = if result.content.is_object() {
                        result.content.clone()
                    } else {
                        json!({"result": result.content})
                    };
                    function.insert("response".into(), response);
                    json!({"functionResponse":function})
                }
                PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => opaque.data.clone(),
                _ => return Err(Error::Unsupported("Gemini 消息不支持此角色或内容块".into())),
            };
            blocks.push(wire::encode_block(part, PROTOCOL, block)?);
        }
        if message.role == Role::Tool
            && wire::append_to_previous_user(&mut output, "parts", &mut blocks)
        {
            continue;
        }
        let mut raw = wire::extra(&message.metadata, PROTOCOL);
        raw.insert("parts".into(), Value::Array(blocks));
        if let Some(role) = role {
            raw.insert("role".into(), json!(role));
        }
        output.push(Value::Object(raw));
    }
    output
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(Into::into))
        .collect()
}
