//! 请求协议类型中的消息和缓存投影。

use crate::{
    ir::request::source::Source,
    ir::{
        cache::CacheSettings,
        message::{Role, ToolCall, ToolResult},
        request::{Instruction, Item, Message},
    },
    protocol::{
        OptionalNullable,
        responses::request::body::{Input, InputItem},
    },
};

use super::{Result, encode_changed};
use crate::adapter::request;

/// 从四种请求类型提取对话消息，跳过 Responses 的独立工具输入项。
pub(super) fn decode_messages(source: &Source) -> Result<Vec<Message>> {
    match source {
        Source::Chat(body) => request::decode_chat(&body.messages),
        Source::Responses(body) => {
            let messages = response_input_messages(&body.input);
            request::decode_responses(&messages)
        }
        Source::Messages(body) => request::decode_messages(&body.messages),
        Source::Gemini(body) => request::decode_gemini(&body.contents),
    }
}

/// 顶层指令独立于消息历史，避免跨协议时丢失权限位置。
pub(super) fn decode_instructions(source: &Source) -> Vec<Instruction> {
    let text = match source {
        Source::Chat(_) => None,
        Source::Responses(body) => match &body.instructions {
            OptionalNullable::Value(text) => Some(text.clone()),
            _ => None,
        },
        Source::Messages(body) => match &body.system {
            OptionalNullable::Value(
                crate::protocol::messages::request::body::SystemPrompt::Text(text),
            ) => Some(text.clone()),
            OptionalNullable::Value(
                crate::protocol::messages::request::body::SystemPrompt::Parts(parts),
            ) => Some(
                parts
                    .iter()
                    .map(|part| part.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        },
        Source::Gemini(body) => match &body.system_instruction {
            OptionalNullable::Value(message) => Some(
                message
                    .parts
                    .iter()
                    .filter_map(|part| match &part.text {
                        OptionalNullable::Value(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        },
    };
    text.map(|text| {
        vec![Instruction {
            role: Role::System,
            text,
        }]
    })
    .unwrap_or_default()
}

/// 提取 Responses 独立函数项，同时保留普通消息的索引顺序。
pub(super) fn decode_items(source: &Source, message_count: usize) -> Result<Vec<Item>> {
    let Source::Responses(body) = source else {
        return Ok((0..message_count).map(Item::Message).collect());
    };
    let OptionalNullable::Value(Input::Items(input)) = &body.input else {
        return Ok(Vec::new());
    };
    let mut items = Vec::with_capacity(input.len());
    let mut index = 0;
    for item in input {
        match item {
            InputItem::Message(_) => {
                items.push(Item::Message(index));
                index += 1;
            }
            InputItem::FunctionCall(raw) => items.push(Item::ToolCall {
                call: ToolCall {
                    id: raw.call_id.as_option().cloned(),
                    name: raw.name.clone(),
                    arguments: serde_json::from_str(&raw.arguments)
                        .unwrap_or_else(|_| serde_json::Value::String(raw.arguments.clone())),
                },
                item_id: raw.id.as_option().cloned(),
            }),
            InputItem::FunctionCallOutput(raw) => items.push(Item::ToolResult(ToolResult {
                id: raw.call_id.as_option().cloned(),
                name: None,
                content: raw.output.clone(),
            })),
            InputItem::Other(raw) => {
                items.push(Item::Opaque(serde_json::Value::Object(raw.clone())))
            }
        }
    }
    Ok(items)
}

/// 将编辑后的消息写回来源请求类型。
pub(super) fn encode_messages(source: &mut Source, edited: &[Message]) -> Result<()> {
    match source {
        Source::Chat(body) => {
            if let Some(messages) = encode_changed(
                &body.messages,
                edited,
                request::decode_chat,
                request::encode_chat,
            )? {
                body.messages = messages;
            }
        }
        Source::Responses(body) => {
            let original = response_input_messages(&body.input);
            if let (Some(messages), OptionalNullable::Value(Input::Items(items))) = (
                encode_changed(
                    &original,
                    edited,
                    request::decode_responses,
                    request::encode_responses,
                )?,
                &mut body.input,
            ) {
                let mut messages = messages.into_iter();
                for item in items {
                    if let InputItem::Message(message) = item {
                        *message = messages.next().expect("消息数量已验证");
                    }
                }
            }
        }
        Source::Messages(body) => {
            if let Some(messages) = encode_changed(
                &body.messages,
                edited,
                request::decode_messages,
                request::encode_messages,
            )? {
                body.messages = messages;
            }
        }
        Source::Gemini(body) => {
            if let Some(messages) = encode_changed(
                &body.contents,
                edited,
                request::decode_gemini,
                request::encode_gemini,
            )? {
                body.contents = messages;
            }
        }
    }
    Ok(())
}

/// 从 Responses 的混合 input 中收集消息，保留原有相对顺序。
fn response_input_messages(
    input: &OptionalNullable<Input>,
) -> Vec<crate::protocol::responses::request::message::Message> {
    let OptionalNullable::Value(Input::Items(items)) = input else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            InputItem::Message(message) => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// 从协议请求类型提取通用缓存设置。
pub(super) fn decode_cache(source: &Source) -> CacheSettings {
    match source {
        Source::Chat(body) => request::decode_chat_cache(body),
        Source::Responses(body) => request::decode_responses_cache(body),
        Source::Messages(body) => request::decode_messages_cache(body),
        Source::Gemini(body) => request::decode_gemini_cache(body),
    }
}

/// 将缓存设置直接写回协议请求类型。
pub(super) fn encode_cache(source: &mut Source, cache: &CacheSettings) {
    match source {
        Source::Chat(body) => request::encode_chat_cache(body, cache),
        Source::Responses(body) => request::encode_responses_cache(body, cache),
        Source::Messages(body) => request::encode_messages_cache(body, cache),
        Source::Gemini(body) => request::encode_gemini_cache(body, cache),
    }
}
