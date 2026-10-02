//! 非流式响应协议类型中的消息和用量投影。

use serde_json::Value;

use crate::{
    ir::{response::Message, response::source::Source, usage::Usage},
    protocol::{OptionalNullable, responses::response::body::OutputItem},
};

use super::{Error, Result, encode_changed};
use crate::adapter::response;

/// 从候选、输出项或顶层消息中提取响应消息。
pub(super) fn decode_messages(source: &Source) -> Result<Vec<Message>> {
    match source {
        Source::Chat(body) => response::decode_chat(
            &body
                .choices
                .iter()
                .map(|choice| choice.message.clone())
                .collect::<Vec<_>>(),
        ),
        Source::Responses(body) => {
            response::decode_responses(&response_output_messages(&body.output))
        }
        Source::Messages(body) => response::decode_messages(std::slice::from_ref(body.as_ref())),
        Source::Gemini(body) => {
            response::decode_gemini(&gemini_candidate_messages(&body.candidates))
        }
    }
}

/// 将编辑后的消息写回来源响应类型，保留候选及非消息输出项。
pub(super) fn encode_messages(source: &mut Source, edited: &[Message]) -> Result<()> {
    match source {
        Source::Chat(body) => {
            let original = body
                .choices
                .iter()
                .map(|choice| choice.message.clone())
                .collect::<Vec<_>>();
            if let Some(messages) = encode_changed(
                &original,
                edited,
                response::decode_chat,
                response::encode_chat,
            )? {
                for (choice, message) in body.choices.iter_mut().zip(messages) {
                    choice.message = message;
                }
            }
        }
        Source::Responses(body) => {
            let original = response_output_messages(&body.output);
            if let Some(messages) = encode_changed(
                &original,
                edited,
                response::decode_responses,
                response::encode_responses,
            )? {
                let mut messages = messages.into_iter();
                for item in &mut body.output {
                    if let OutputItem::Message(message) = item {
                        *message = messages.next().expect("消息数量已验证");
                    }
                }
            }
        }
        Source::Messages(body) => {
            if let Some(messages) = encode_changed(
                std::slice::from_ref(body.as_ref()),
                edited,
                response::decode_messages,
                response::encode_messages,
            )? {
                **body = messages.into_iter().next().expect("顶层响应只有一条消息");
            }
        }
        Source::Gemini(body) => {
            let original = gemini_candidate_messages(&body.candidates);
            if let Some(messages) = encode_changed(
                &original,
                edited,
                response::decode_gemini,
                response::encode_gemini,
            )? {
                let mut messages = messages.into_iter();
                if let OptionalNullable::Value(candidates) = &mut body.candidates {
                    for candidate in candidates {
                        if let OptionalNullable::Value(content) = &mut candidate.content {
                            *content = messages.next().expect("消息数量已验证");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// 收集 Responses 中的消息输出项，排除函数调用等其他输出项。
fn response_output_messages(
    output: &[OutputItem],
) -> Vec<crate::protocol::responses::response::message::Message> {
    output
        .iter()
        .filter_map(|item| match item {
            OutputItem::Message(message) => Some(message.clone()),
            OutputItem::Other(_) => None,
        })
        .collect()
}

/// 收集 Gemini 中有内容的候选。
fn gemini_candidate_messages(
    candidates: &OptionalNullable<Vec<crate::protocol::gemini::response::body::Candidate>>,
) -> Vec<crate::protocol::gemini::response::message::Message> {
    let OptionalNullable::Value(candidates) = candidates else {
        return Vec::new();
    };
    candidates
        .iter()
        .filter_map(|candidate| match &candidate.content {
            OptionalNullable::Value(content) => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// 从协议响应类型提取用量和缓存计数。
pub(super) fn decode_usage(source: &Source) -> Option<Usage> {
    match source {
        Source::Chat(body) => present(&body.usage).map(response::decode_chat_usage),
        Source::Responses(body) => present(&body.usage).map(response::decode_responses_usage),
        Source::Messages(body) => Some(response::decode_messages_usage(&body.usage)),
        Source::Gemini(body) => present(&body.usage_metadata).map(response::decode_gemini_usage),
    }
}

/// 将用量直接写回协议响应类型；Messages 的用量必填。
pub(super) fn encode_usage(source: &mut Source, usage: Option<&Usage>) -> Result<()> {
    match source {
        Source::Chat(body) => {
            body.usage = match usage {
                Some(usage) => OptionalNullable::Value(response::encode_chat_usage(
                    usage,
                    present(&body.usage),
                )?),
                None => OptionalNullable::Missing,
            };
        }
        Source::Responses(body) => {
            body.usage = match usage {
                Some(usage) => OptionalNullable::Value(response::encode_responses_usage(
                    usage,
                    present(&body.usage),
                )?),
                None => OptionalNullable::Missing,
            };
        }
        Source::Messages(body) => {
            let usage =
                usage.ok_or_else(|| Error::Unsupported("Messages 响应必须包含 usage".into()))?;
            body.usage = response::encode_messages_usage(usage, Some(&body.usage))?;
        }
        Source::Gemini(body) => {
            body.usage_metadata = match usage {
                Some(usage) => OptionalNullable::Value(response::encode_gemini_usage(
                    usage,
                    present(&body.usage_metadata),
                )),
                None => OptionalNullable::Missing,
            };
        }
    }
    Ok(())
}

/// 返回确有值的协议可空字段。
fn present<T>(value: &OptionalNullable<T>) -> Option<&T> {
    match value {
        OptionalNullable::Value(value) => Some(value),
        _ => None,
    }
}

/// 派生计数可能随缓存修改而变化；只验证调用方实际编辑过的字段。
pub(super) fn changed_usage_fields_match(
    original: &Option<Usage>,
    requested: &Option<Usage>,
    encoded: &Option<Usage>,
) -> Result<bool> {
    Ok(changed_fields_match(
        &serde_json::to_value(original)?,
        &serde_json::to_value(requested)?,
        &serde_json::to_value(encoded)?,
    ))
}

/// 递归比较对象中发生变化的叶子字段。
fn changed_fields_match(original: &Value, requested: &Value, encoded: &Value) -> bool {
    if original == requested {
        return true;
    }
    match (original, requested, encoded) {
        (Value::Object(before), Value::Object(after), Value::Object(actual)) => {
            after.iter().all(|(key, value)| {
                changed_fields_match(
                    before.get(key).unwrap_or(&Value::Null),
                    value,
                    actual.get(key).unwrap_or(&Value::Null),
                )
            })
        }
        _ => requested == encoded,
    }
}
