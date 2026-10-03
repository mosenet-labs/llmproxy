//! 从 Messages 请求类型直接提取内容。
use super::super::{Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, PartKind, Role, ToolCall, ToolResult},
    protocol::messages::request::message::{
        Content, ContentBlock, KnownContentBlock as Block, Message, Role as RawRole,
    },
};
use serde_json::{Map, Value};
/// 保持字符串与块数组形状，逐个读取已知块字段。
pub fn decode_messages(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                RawRole::User => Role::User,
                RawRole::Assistant => Role::Assistant,
                RawRole::System => Role::System,
            };
            let (form, parts) = match &m.content {
                Content::Text(text) => (
                    "text",
                    vec![wire::text_part(
                        text.clone(),
                        PROTOCOL,
                        "scalar",
                        Map::new(),
                    )],
                ),
                Content::Parts(blocks) => {
                    let parts = blocks
                        .iter()
                        .map(|block| match block {
                            ContentBlock::Known(Block::Text {
                                text,
                                cache_control,
                                citations,
                                extra,
                            }) => {
                                let mut extra = extra.clone();
                                wire::put(&mut extra, "cache_control", cache_control)?;
                                wire::put(&mut extra, "citations", citations)?;
                                Ok(wire::text_part(text.clone(), PROTOCOL, "text", extra))
                            }
                            ContentBlock::Known(Block::ToolUse {
                                id,
                                name,
                                input,
                                cache_control,
                                caller,
                                toolset_name,
                                extra,
                            }) => {
                                let mut extra = extra.clone();
                                wire::put(&mut extra, "cache_control", cache_control)?;
                                wire::put(&mut extra, "caller", caller)?;
                                wire::put(&mut extra, "toolset_name", toolset_name)?;
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
                            ContentBlock::Known(Block::ToolResult {
                                tool_use_id,
                                content,
                                cache_control,
                                is_error,
                                toolset_name,
                                extra,
                            }) => {
                                let mut extra = extra.clone();
                                wire::put(&mut extra, "cache_control", cache_control)?;
                                wire::put(&mut extra, "is_error", is_error)?;
                                wire::put(&mut extra, "toolset_name", toolset_name)?;
                                let value = content
                                    .as_option()
                                    .map(serde_json::to_value)
                                    .transpose()?
                                    .unwrap_or(Value::Null);
                                if value.is_null() {
                                    wire::put(&mut extra, "content", content)?;
                                }
                                Ok(wire::part(
                                    PartKind::ToolResult(ToolResult {
                                        id: Some(tool_use_id.clone()),
                                        name: None,
                                        content: value,
                                    }),
                                    PROTOCOL,
                                    "tool_result",
                                    extra,
                                ))
                            }
                            _ => wire::opaque_value(PROTOCOL, block),
                        })
                        .collect::<Result<_>>()?;
                    ("parts", parts)
                }
            };
            Ok(wire::message(role, parts, PROTOCOL, form, m.extra.clone()))
        })
        .collect()
}
