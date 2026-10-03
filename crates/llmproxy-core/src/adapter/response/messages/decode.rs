//! Messages 响应消息的直接类型投影。
use super::super::{Result, wire};
use super::PROTOCOL;
use crate::{
    ir::response::{Message as IrMessage, PartKind, Role, ToolCall},
    protocol::messages::response::message::{ContentBlock, KnownContentBlock as Block, Message},
};
use serde_json::Value;
/// 顶层外壳保留为叶子元数据，内容块直接访问类型字段。
pub fn decode_messages(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|m| {
            let mut extra = m.extra.clone();
            extra.insert("id".into(), Value::String(m.id.clone()));
            extra.insert("model".into(), Value::String(m.model.clone()));
            extra.insert("type".into(), serde_json::to_value(m.r#type)?);
            extra.insert("usage".into(), serde_json::to_value(&m.usage)?);
            wire::put(&mut extra, "container", &m.container)?;
            wire::put(&mut extra, "diagnostics", &m.diagnostics)?;
            wire::put(&mut extra, "stop_details", &m.stop_details)?;
            wire::put(&mut extra, "stop_reason", &m.stop_reason)?;
            wire::put(&mut extra, "stop_sequence", &m.stop_sequence)?;

            let parts = m
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Known(Block::Text {
                        text,
                        citations,
                        extra,
                    }) => {
                        let mut extra = extra.clone();
                        wire::put(&mut extra, "citations", citations)?;
                        Ok(wire::text_part(text.clone(), PROTOCOL, "text", extra))
                    }
                    ContentBlock::Known(Block::ToolUse {
                        id,
                        name,
                        input,
                        caller,
                        extra,
                    }) if caller.as_option().is_none_or(|caller| {
                        caller.is_null()
                            || caller.get("type").and_then(Value::as_str) == Some("direct")
                    }) =>
                    {
                        let mut extra = extra.clone();
                        wire::put(&mut extra, "caller", caller)?;
                        Ok(wire::part(
                            PartKind::ToolCall(ToolCall {
                                id: Some(id.clone()),
                                name: name.clone(),
                                arguments: Value::Object(input.clone()),
                            }),
                            PROTOCOL,
                            "tool_use",
                            extra,
                        ))
                    }
                    ContentBlock::Known(Block::Thinking {
                        thinking,
                        signature,
                        extra,
                    }) => {
                        let mut extra = extra.clone();
                        extra.insert("signature".into(), Value::String(signature.clone()));
                        Ok(wire::part(
                            PartKind::Reasoning(Value::String(thinking.clone())),
                            PROTOCOL,
                            "thinking",
                            extra,
                        ))
                    }
                    _ => wire::opaque_value(PROTOCOL, block),
                })
                .collect::<Result<_>>()?;
            Ok(wire::response_message(
                Role::Assistant,
                parts,
                PROTOCOL,
                "parts",
                extra,
            ))
        })
        .collect()
}
