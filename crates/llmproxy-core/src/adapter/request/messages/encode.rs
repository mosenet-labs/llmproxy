//! IR 消息直接构造 Messages 内容块。
use super::super::{Error, Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::{
        OptionalNullable as O,
        messages::request::message::{
            Content, ContentBlock, KnownContentBlock as Block, Message, Role as RawRole,
            ToolResultContent,
        },
    },
};
/// 合并独立工具结果与前一条用户消息，保留原始片段顺序。
pub fn encode_messages(messages: &[IrMessage]) -> Result<Vec<Message>> {
    let mut output: Vec<Message> = Vec::new();
    for message in messages {
        wire::reject_unmapped_source(message, PROTOCOL)?;
        let role = match message.role {
            Role::User | Role::Tool => RawRole::User,
            Role::Assistant => RawRole::Assistant,
            Role::System => RawRole::System,
            role => return Err(wire::unsupported_role(role)),
        };
        let mut blocks = Vec::new();
        for part in &message.parts {
            let mut extra = wire::extra(&part.metadata, PROTOCOL);
            let block = match &part.kind {
                PartKind::Text(text) => Block::Text {
                    text: text.clone(),
                    cache_control: wire::take(&mut extra, "cache_control")?,
                    citations: wire::take(&mut extra, "citations")?,
                    extra,
                },
                PartKind::ToolCall(call) if role == RawRole::Assistant => Block::ToolUse {
                    id: call
                        .id
                        .clone()
                        .ok_or_else(|| Error::Unsupported("Messages 工具调用缺少 ID".into()))?,
                    name: call.name.clone(),
                    input: call.arguments.as_object().cloned().ok_or_else(|| {
                        Error::Unsupported("Messages 工具参数必须是 JSON 对象".into())
                    })?,
                    cache_control: wire::take(&mut extra, "cache_control")?,
                    caller: wire::take(&mut extra, "caller")?,
                    toolset_name: wire::take(&mut extra, "toolset_name")?,
                    extra,
                },
                PartKind::ToolResult(result) if role == RawRole::User => {
                    let original_content = wire::take(&mut extra, "content")?;
                    let content = if result.content.is_null() {
                        original_content
                    } else if result.content.is_object() {
                        O::Value(ToolResultContent::Text(serde_json::to_string(
                            &result.content,
                        )?))
                    } else {
                        O::Value(serde_json::from_value(result.content.clone())?)
                    };
                    Block::ToolResult {
                        tool_use_id: result
                            .id
                            .clone()
                            .ok_or_else(|| Error::Unsupported("Messages 工具结果缺少 ID".into()))?,
                        content,
                        cache_control: wire::take(&mut extra, "cache_control")?,
                        is_error: wire::take(&mut extra, "is_error")?,
                        toolset_name: wire::take(&mut extra, "toolset_name")?,
                        extra,
                    }
                }
                PartKind::Media(media) => {
                    if let crate::ir::media::OriginalMedia::Messages(part) =
                        crate::adapter::media::encode(media, PROTOCOL)?
                    {
                        blocks.push(part);
                    }
                    continue;
                }
                PartKind::Reasoning(value) => Block::Thinking {
                    thinking: value
                        .as_str()
                        .ok_or_else(|| Error::Unsupported("思考正文必须是文本".into()))?
                        .into(),
                    signature: wire::required(&mut extra, "signature")?,
                    extra,
                },
                PartKind::ServerOutput(output) => {
                    blocks.push(serde_json::from_value(
                        crate::adapter::server_output::original(output, PROTOCOL)?.clone(),
                    )?);
                    continue;
                }
                PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                    blocks.push(serde_json::from_value(opaque.data.clone())?);
                    continue;
                }
                _ => return Err(Error::Unsupported("Messages 消息不支持此内容块".into())),
            };
            blocks.push(ContentBlock::Known(block));
        }
        let form = wire::form(&message.metadata, PROTOCOL);
        if message.role == Role::Tool
            && form.is_none()
            && let Some(Message {
                role: RawRole::User,
                content: Content::Parts(previous),
                ..
            }) = output.last_mut()
        {
            previous.append(&mut blocks);
            continue;
        }
        let content = if form == Some("text")
            && let [ContentBlock::Known(Block::Text { text, .. })] = blocks.as_slice()
        {
            Content::Text(text.clone())
        } else {
            Content::Parts(blocks)
        };
        output.push(Message {
            role,
            content,
            extra: wire::extra(&message.metadata, PROTOCOL),
        });
    }
    Ok(output)
}
