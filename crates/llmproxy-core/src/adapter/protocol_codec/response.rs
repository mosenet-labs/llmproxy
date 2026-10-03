//! 非流式响应协议类型中的消息和用量投影。

use serde_json::Value;

use crate::{
    ir::{
        message::ToolCall,
        response::source::Source,
        response::{Item, Message},
        usage::Usage,
    },
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

/// 提取 Responses 独立函数输出项，保留与文本消息的相对顺序。
pub(super) fn decode_items(source: &Source, message_count: usize) -> Result<Vec<Item>> {
    let Source::Responses(body) = source else {
        return Ok((0..message_count).map(Item::Message).collect());
    };
    let mut items = Vec::with_capacity(body.output.len());
    let mut index = 0;
    for item in &body.output {
        match item {
            OutputItem::Message(_) => {
                items.push(Item::Message(index));
                index += 1;
            }
            OutputItem::FunctionCall(raw) => items.push(Item::ToolCall {
                call: ToolCall {
                    id: raw.call_id.as_option().cloned(),
                    name: raw.name.clone(),
                    arguments: serde_json::from_str(&raw.arguments)
                        .unwrap_or_else(|_| Value::String(raw.arguments.clone())),
                },
                item_id: raw.id.as_option().cloned(),
            }),
            OutputItem::Other(raw)
                if raw.get("type").and_then(serde_json::Value::as_str) == Some("reasoning") =>
            {
                let summary = raw
                    .get("summary")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !summary.is_empty() {
                    items.push(Item::Reasoning(summary));
                } else {
                    items.push(Item::Opaque(serde_json::Value::Object(raw.clone())));
                }
            }
            OutputItem::Other(raw) => items.push(
                crate::adapter::server_output::decode(
                    crate::protocol::Protocol::OpenAiResponses,
                    raw,
                )
                .map(Item::ServerOutput)
                .unwrap_or_else(|| Item::Opaque(Value::Object(raw.clone()))),
            ),
        }
    }
    Ok(items)
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
            _ => None,
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
) -> bool {
    let (Some(before), Some(after), Some(actual)) = (original, requested, encoded) else {
        return original == requested || requested == encoded;
    };
    [
        (before.input_tokens, after.input_tokens, actual.input_tokens),
        (
            before.output_tokens,
            after.output_tokens,
            actual.output_tokens,
        ),
        (before.total_tokens, after.total_tokens, actual.total_tokens),
        (
            before.cache.read_input_tokens,
            after.cache.read_input_tokens,
            actual.cache.read_input_tokens,
        ),
        (
            before.cache.write_input_tokens,
            after.cache.write_input_tokens,
            actual.cache.write_input_tokens,
        ),
        (
            before.cache.write_short_input_tokens,
            after.cache.write_short_input_tokens,
            actual.cache.write_short_input_tokens,
        ),
        (
            before.cache.write_long_input_tokens,
            after.cache.write_long_input_tokens,
            actual.cache.write_long_input_tokens,
        ),
        (
            before.input_details.uncached_tokens,
            after.input_details.uncached_tokens,
            actual.input_details.uncached_tokens,
        ),
        (
            before.input_details.text_tokens,
            after.input_details.text_tokens,
            actual.input_details.text_tokens,
        ),
        (
            before.input_details.audio_tokens,
            after.input_details.audio_tokens,
            actual.input_details.audio_tokens,
        ),
        (
            before.input_details.image_tokens,
            after.input_details.image_tokens,
            actual.input_details.image_tokens,
        ),
        (
            before.input_details.video_tokens,
            after.input_details.video_tokens,
            actual.input_details.video_tokens,
        ),
        (
            before.input_details.document_tokens,
            after.input_details.document_tokens,
            actual.input_details.document_tokens,
        ),
        (
            before.input_details.tool_tokens,
            after.input_details.tool_tokens,
            actual.input_details.tool_tokens,
        ),
        (
            before.output_details.text_tokens,
            after.output_details.text_tokens,
            actual.output_details.text_tokens,
        ),
        (
            before.output_details.audio_tokens,
            after.output_details.audio_tokens,
            actual.output_details.audio_tokens,
        ),
        (
            before.output_details.image_tokens,
            after.output_details.image_tokens,
            actual.output_details.image_tokens,
        ),
        (
            before.output_details.video_tokens,
            after.output_details.video_tokens,
            actual.output_details.video_tokens,
        ),
        (
            before.output_details.reasoning_tokens,
            after.output_details.reasoning_tokens,
            actual.output_details.reasoning_tokens,
        ),
        (
            before.output_details.accepted_prediction_tokens,
            after.output_details.accepted_prediction_tokens,
            actual.output_details.accepted_prediction_tokens,
        ),
        (
            before.output_details.rejected_prediction_tokens,
            after.output_details.rejected_prediction_tokens,
            actual.output_details.rejected_prediction_tokens,
        ),
    ]
    .into_iter()
    .all(|(before, after, actual)| before == after || after == actual)
}
