//! 从 IR 直接构造 Responses 消息变体。
use super::super::{Error, Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, PartKind, Role},
    protocol::responses::request::message::{self as raw, Message},
};
/// 输入与输出内容块分别构造，不先拼 JSON 再解释变体。
pub fn encode_responses(messages: &[IrMessage]) -> Result<Vec<Message>> {
    messages
        .iter()
        .map(|message| {
            wire::reject_unmapped_source(message, PROTOCOL)?;
            let mut extra = wire::extra(&message.metadata, PROTOCOL);
            let form = wire::form(&message.metadata, PROTOCOL).unwrap_or("");
            let role = match message.role {
                Role::User => raw::Role::User,
                Role::Assistant => raw::Role::Assistant,
                Role::System => raw::Role::System,
                Role::Developer => raw::Role::Developer,
                role => return Err(wire::unsupported_role(role)),
            };
            if form.starts_with("output_") {
                if message.role != Role::Assistant {
                    return Err(wire::unsupported_role(message.role));
                }
                let content = message
                    .parts
                    .iter()
                    .map(|part| {
                        let mut extra = wire::extra(&part.metadata, PROTOCOL);
                        Ok(match &part.kind {
                            PartKind::Text(text) => raw::OutputPart::OutputText {
                                text: text.clone(),
                                annotations: wire::take_option(&mut extra, "annotations")?
                                    .unwrap_or_default(),
                                logprobs: wire::take(&mut extra, "logprobs")?,
                                extra,
                            },
                            PartKind::Refusal(text) => raw::OutputPart::Refusal {
                                refusal: text.clone(),
                                extra,
                            },
                            PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                                serde_json::from_value(opaque.data.clone())?
                            }
                            _ => {
                                return Err(Error::Unsupported(
                                    "Responses 输出消息不支持此内容块".into(),
                                ));
                            }
                        })
                    })
                    .collect::<Result<_>>()?;
                return Ok(Message::Output(raw::OutputMessage {
                    id: wire::required(&mut extra, "id")?,
                    status: wire::required(&mut extra, "status")?,
                    r#type: wire::required(&mut extra, "type")?,
                    role: raw::AssistantRole::Assistant,
                    content,
                    phase: wire::take(&mut extra, "phase")?,
                    extra,
                }));
            }
            let parts = message
                .parts
                .iter()
                .map(|part| {
                    Ok(match &part.kind {
                        PartKind::Text(text) => raw::InputPart::InputText {
                            text: text.clone(),
                            extra: wire::extra(&part.metadata, PROTOCOL),
                        },
                        PartKind::Media(media) => {
                            match crate::adapter::media::encode(media, PROTOCOL)? {
                                crate::ir::media::OriginalMedia::Responses(part) => part,
                                _ => unreachable!(),
                            }
                        }
                        PartKind::Opaque(opaque) if opaque.protocol == PROTOCOL => {
                            serde_json::from_value(opaque.data.clone())?
                        }
                        _ => {
                            return Err(Error::Unsupported(
                                "Responses 输入消息不支持此内容块".into(),
                            ));
                        }
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if form.starts_with("input_") {
                let role = match role {
                    raw::Role::User => raw::InputRole::User,
                    raw::Role::System => raw::InputRole::System,
                    raw::Role::Developer => raw::InputRole::Developer,
                    _ => return Err(wire::unsupported_role(message.role)),
                };
                Ok(Message::Input(raw::InputMessage {
                    role,
                    content: parts,
                    status: wire::take_option(&mut extra, "status")?,
                    r#type: wire::take_option(&mut extra, "type")?,
                    extra,
                }))
            } else {
                let content = if form.ends_with("_text")
                    && let [raw::InputPart::InputText { text, .. }] = parts.as_slice()
                {
                    raw::Content::Text(text.clone())
                } else {
                    raw::Content::Parts(parts)
                };
                Ok(Message::Easy(raw::EasyInputMessage {
                    role,
                    content,
                    phase: wire::take(&mut extra, "phase")?,
                    r#type: wire::take_option(&mut extra, "type")?,
                    extra,
                }))
            }
        })
        .collect()
}
