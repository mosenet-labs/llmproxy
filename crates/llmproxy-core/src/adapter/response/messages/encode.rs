//! IR 响应消息直接构造 Messages 类型。
use super::super::{Error, Result, reject_unmapped_chat, reject_unmapped_parts, wire};
use super::PROTOCOL;
use crate::{
    ir::response::{Message as IrMessage, PartKind, Role},
    protocol::messages::response::message::{
        AssistantRole, ContentBlock, KnownContentBlock as Block, Message,
    },
};
/// 响应外壳必须由来源元数据提供，消息适配器不伪造 ID、模型或用量。
pub fn encode_messages(messages: &[IrMessage]) -> Result<Vec<Message>> {
    messages
        .iter()
        .map(|message| {
            reject_unmapped_chat(message, PROTOCOL)?;
            reject_unmapped_parts(message, PROTOCOL)?;
            if message.role != Role::Assistant {
                return Err(wire::unsupported_role(message.role));
            }
            let mut extra = wire::extra(&message.metadata, PROTOCOL);
            let content = message
                .parts
                .iter()
                .map(|part| {
                    let mut extra = wire::extra(&part.metadata, PROTOCOL);
                    Ok(match &part.kind {
                        PartKind::Text(text) => ContentBlock::Known(Block::Text {
                            text: text.clone(),
                            citations: wire::take(&mut extra, "citations")?,
                            extra,
                        }),
                        PartKind::ToolCall(call) => ContentBlock::Known(Block::ToolUse {
                            id: call
                                .id
                                .clone()
                                .ok_or_else(|| Error::Unsupported("工具调用缺少 ID".into()))?,
                            name: call.name.clone(),
                            input: call.arguments.as_object().cloned().ok_or_else(|| {
                                Error::Unsupported("工具参数必须是 JSON 对象".into())
                            })?,
                            caller: wire::take(&mut extra, "caller")?,
                            extra,
                        }),
                        PartKind::Reasoning(value) => ContentBlock::Known(Block::Thinking {
                            thinking: value
                                .as_str()
                                .ok_or_else(|| Error::Unsupported("思考正文必须是文本".into()))?
                                .into(),
                            signature: wire::required(&mut extra, "signature")?,
                            extra,
                        }),
                        PartKind::ServerOutput(output) => serde_json::from_value(
                            crate::adapter::server_output::original(output, PROTOCOL)?.clone(),
                        )?,
                        PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                            serde_json::from_value(opaque.data.clone())?
                        }
                        _ => return Err(Error::Unsupported("Messages 响应不支持此内容块".into())),
                    })
                })
                .collect::<Result<_>>()?;
            Ok(Message {
                id: wire::required(&mut extra, "id")?,
                model: wire::required(&mut extra, "model")?,
                r#type: wire::required(&mut extra, "type")?,
                usage: wire::required(&mut extra, "usage")?,
                role: AssistantRole::Assistant,
                content,
                container: wire::take(&mut extra, "container")?,
                diagnostics: wire::take(&mut extra, "diagnostics")?,
                stop_details: wire::take(&mut extra, "stop_details")?,
                stop_reason: wire::take(&mut extra, "stop_reason")?,
                stop_sequence: wire::take(&mut extra, "stop_sequence")?,
                extra,
            })
        })
        .collect()
}
