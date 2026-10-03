//! Responses 消息类型的直接投影。
use super::super::{Result, wire};
use super::PROTOCOL;
use crate::{
    ir::request::{Message as IrMessage, Part, PartKind, Role},
    protocol::responses::request::message::*,
};
use serde_json::{Map, Value};
/// 保留 Easy、Input、Output 变体及内容形状。
pub fn decode_responses(messages: &[Message]) -> Result<Vec<IrMessage>> {
    messages
        .iter()
        .map(|message| {
            let (role, parts, form, extra) = match message {
                Message::Easy(m) => {
                    let mut extra = m.extra.clone();
                    wire::put(&mut extra, "phase", &m.phase)?;
                    wire::put_option(&mut extra, "type", &m.r#type)?;
                    let role = match m.role {
                        crate::protocol::responses::request::message::Role::User => Role::User,
                        crate::protocol::responses::request::message::Role::Assistant => {
                            Role::Assistant
                        }
                        crate::protocol::responses::request::message::Role::System => Role::System,
                        crate::protocol::responses::request::message::Role::Developer => {
                            Role::Developer
                        }
                    };
                    let (form, parts) = match &m.content {
                        Content::Text(text) => (
                            "easy_text",
                            vec![wire::text_part(
                                text.clone(),
                                PROTOCOL,
                                "scalar",
                                Map::new(),
                            )],
                        ),
                        Content::Parts(parts) => ("easy_parts", input_parts(parts)?),
                    };
                    (role, parts, form, extra)
                }
                Message::Input(m) => {
                    let mut extra = m.extra.clone();
                    wire::put_option(&mut extra, "status", &m.status)?;
                    wire::put_option(&mut extra, "type", &m.r#type)?;
                    let role = match m.role {
                        InputRole::User => Role::User,
                        InputRole::System => Role::System,
                        InputRole::Developer => Role::Developer,
                    };
                    (role, input_parts(&m.content)?, "input_parts", extra)
                }
                Message::Output(m) => {
                    let mut extra = m.extra.clone();
                    extra.insert("id".into(), Value::String(m.id.clone()));
                    extra.insert("status".into(), serde_json::to_value(m.status)?);
                    extra.insert("type".into(), serde_json::to_value(m.r#type)?);
                    wire::put(&mut extra, "phase", &m.phase)?;
                    let parts = m
                        .content
                        .iter()
                        .map(|part| match part {
                            OutputPart::OutputText {
                                text,
                                annotations,
                                logprobs,
                                extra,
                            } => {
                                let mut extra = extra.clone();
                                extra.insert(
                                    "annotations".into(),
                                    Value::Array(annotations.clone()),
                                );
                                wire::put_option(&mut extra, "logprobs", logprobs)?;
                                Ok(wire::text_part(
                                    text.clone(),
                                    PROTOCOL,
                                    "output_text",
                                    extra,
                                ))
                            }
                            OutputPart::Refusal { refusal, extra } => Ok(wire::part(
                                PartKind::Refusal(refusal.clone()),
                                PROTOCOL,
                                "refusal",
                                extra.clone(),
                            )),
                        })
                        .collect::<Result<_>>()?;
                    (Role::Assistant, parts, "output_parts", extra)
                }
            };
            Ok(wire::message(role, parts, PROTOCOL, form, extra))
        })
        .collect()
}
/// 输入文本直接提取；未规范化媒体作为不透明叶子保留。
fn input_parts(parts: &[InputPart]) -> Result<Vec<Part>> {
    parts
        .iter()
        .map(|part| match part {
            InputPart::InputText { text, extra } => Ok(wire::text_part(
                text.clone(),
                PROTOCOL,
                "input_text",
                extra.clone(),
            )),
            _ => crate::adapter::media::part(crate::ir::media::OriginalMedia::Responses(
                part.clone(),
            )),
        })
        .collect()
}
