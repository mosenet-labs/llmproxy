use llmproxy_core::protocol::{
    OptionalNullable, Protocol, Request, chat::request as chat, gemini::request as gemini,
    messages::request as messages, responses::request as responses,
};
use reqwest::RequestBuilder;
use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};
/// 从已完成的对话记录直接构造协议请求；Gemini 的模型和流式模式由 URL 指定。
pub(super) fn request_body(
    protocol: Protocol,
    alias: &str,
    history: &[ChatMessage],
    stream: bool,
) -> Request {
    let history = history.iter().filter(|message| {
        message.status == ChatMessageStatus::Complete && !message.content.is_empty()
    });
    match protocol {
        Protocol::OpenAiChat => Request::Chat(Box::new(chat::Request {
            model: alias.to_owned(),
            max_completion_tokens: OptionalNullable::Value(2048),
            stream: OptionalNullable::Value(stream),
            messages: history
                .map(|message| {
                    if message.role == ChatBubbleRole::User {
                        chat::Message::User(chat::ContentMessage {
                            content: chat::Content::Text(message.content.clone()),
                            name: None,
                            extra: Default::default(),
                        })
                    } else {
                        chat::Message::Assistant {
                            audio: OptionalNullable::Missing,
                            content: OptionalNullable::Value(chat::Content::Text(
                                message.content.clone(),
                            )),
                            function_call: OptionalNullable::Missing,
                            name: None,
                            refusal: OptionalNullable::Missing,
                            tool_calls: None,
                            extra: Default::default(),
                        }
                    }
                })
                .collect(),
            ..Default::default()
        })),
        Protocol::OpenAiResponses => Request::Responses(Box::new(responses::Request {
            model: OptionalNullable::Value(alias.to_owned()),
            max_output_tokens: OptionalNullable::Value(2048),
            stream: OptionalNullable::Value(stream),
            input: OptionalNullable::Value(responses::body::Input::Items(
                history
                    .map(|message| {
                        responses::body::InputItem::Message(responses::Message::Easy(
                            responses::EasyInputMessage {
                                content: responses::Content::Text(message.content.clone()),
                                role: if message.role == ChatBubbleRole::User {
                                    responses::Role::User
                                } else {
                                    responses::Role::Assistant
                                },
                                phase: OptionalNullable::Missing,
                                r#type: None,
                                extra: Default::default(),
                            },
                        ))
                    })
                    .collect(),
            )),
            ..Default::default()
        })),
        Protocol::AnthropicMessages => Request::Messages(Box::new(messages::Request {
            model: alias.to_owned(),
            stream: OptionalNullable::Value(stream),
            max_tokens: 2048,
            messages: history
                .map(|message| messages::Message {
                    content: messages::Content::Text(message.content.clone()),
                    role: if message.role == ChatBubbleRole::User {
                        messages::Role::User
                    } else {
                        messages::Role::Assistant
                    },
                    extra: Default::default(),
                })
                .collect(),
            ..Default::default()
        })),
        Protocol::Gemini => Request::Gemini(Box::new(gemini::Request {
            generation_config: OptionalNullable::Value(gemini::body::GenerationConfig {
                max_output_tokens: OptionalNullable::Value(2048),
                ..Default::default()
            }),
            contents: history
                .map(|message| gemini::Message {
                    parts: vec![gemini::Part {
                        text: OptionalNullable::Value(message.content.clone()),
                        ..Default::default()
                    }],
                    role: Some(if message.role == ChatBubbleRole::User {
                        gemini::Role::User
                    } else {
                        gemini::Role::Model
                    }),
                    extra: Default::default(),
                })
                .collect(),
            ..Default::default()
        })),
    }
}

/// 在 HTTP 边界序列化具体协议结构，避免将统一载体的枚举标签写入请求正文。
pub(super) fn with_request_body(builder: RequestBuilder, body: &Request) -> RequestBuilder {
    match body {
        Request::Chat(body) => builder.json(body),
        Request::Responses(body) => builder.json(body),
        Request::Messages(body) => builder.json(body),
        Request::Gemini(body) => builder.json(body),
    }
}
