//! Chat 响应消息直接类型投影。
use super::super::{Result, wire};
use super::PROTOCOL;
use crate::{
    ir::response::{Message as IrMessage, PartKind, Role},
    protocol::{OptionalNullable as O, chat::response::message::Message},
};
use serde_json::{Map, Value};
/// 正文、拒绝和工具调用读取类型字段；仅剩余叶子保存在元数据。
pub fn decode_chat(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|m| {
            let mut extra = m.extra.clone();
            wire::put(&mut extra, "annotations", &m.annotations)?;
            wire::put(&mut extra, "audio", &m.audio)?;
            wire::put(&mut extra, "function_call", &m.function_call)?;
            let mut parts = Vec::new();
            if let Some(Value::String(text)) = extra.get("reasoning_content") {
                parts.push(wire::part(
                    PartKind::Reasoning(Value::String(text.clone())),
                    PROTOCOL,
                    "reasoning_content",
                    Map::new(),
                ));
                extra.remove("reasoning_content");
            }
            let form = match &m.content {
                O::Value(text) => {
                    parts.push(wire::text_part(
                        text.clone(),
                        PROTOCOL,
                        "content",
                        Map::new(),
                    ));
                    "text"
                }
                O::Null => {
                    extra.insert("content".into(), Value::Null);
                    "null"
                }
                O::Missing => "missing",
            };
            match &m.refusal {
                O::Value(text) => parts.push(wire::part(
                    PartKind::Refusal(text.clone()),
                    PROTOCOL,
                    "refusal",
                    Map::new(),
                )),
                _ => wire::put(&mut extra, "refusal", &m.refusal)?,
            }
            match &m.tool_calls {
                O::Value(calls) if !calls.is_empty() => {
                    for call in calls {
                        parts.push(crate::adapter::request::chat::decode_call(call)?);
                    }
                }
                _ => wire::put(&mut extra, "tool_calls", &m.tool_calls)?,
            }
            Ok(wire::response_message(
                Role::Assistant,
                parts,
                PROTOCOL,
                form,
                extra,
            ))
        })
        .collect()
}
