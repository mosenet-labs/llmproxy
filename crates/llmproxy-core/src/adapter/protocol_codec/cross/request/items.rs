//! 根据目标协议构造类型化输入项，连续工具块按协议规则合并。
use super::super::unsupported;
use crate::{
    adapter::Result,
    ir::message::Role,
    protocol::{
        self, OptionalNullable as O, Protocol,
        chat::request::{body as cb, message as c},
        gemini::request::{body as gb, message as g},
        messages::request::{body as mb, message as m},
        responses::{
            function as rf,
            request::{body as rb, message as rm},
        },
    },
};
use serde_json::{Map, Value};

/// 四种输入项容器，避免不同协议项混入同一个 JSON 数组。
pub(super) enum Items {
    Chat(Vec<c::Message>),
    Responses(Vec<rb::InputItem>),
    Messages(Vec<m::Message>),
    Gemini(Vec<g::Message>),
}
impl Items {
    /// 按目标协议创建空输入容器。
    pub(super) fn new(protocol: Protocol) -> Self {
        match protocol {
            Protocol::OpenAiChat => Self::Chat(vec![]),
            Protocol::OpenAiResponses => Self::Responses(vec![]),
            Protocol::AnthropicMessages => Self::Messages(vec![]),
            Protocol::Gemini => Self::Gemini(vec![]),
        }
    }
    /// 将高优先级前缀与有序对话拼接。
    pub(super) fn extend(&mut self, other: Self) {
        match (self, other) {
            (Self::Chat(a), Self::Chat(b)) => a.extend(b),
            (Self::Responses(a), Self::Responses(b)) => a.extend(b),
            (Self::Messages(a), Self::Messages(b)) => a.extend(b),
            (Self::Gemini(a), Self::Gemini(b)) => a.extend(b),
            _ => unreachable!("目标协议固定"),
        }
    }
    /// 转换后的历史不能为空。
    pub(super) fn is_empty(&self) -> bool {
        match self {
            Self::Chat(v) => v.is_empty(),
            Self::Responses(v) => v.is_empty(),
            Self::Messages(v) => v.is_empty(),
            Self::Gemini(v) => v.is_empty(),
        }
    }
    /// 写入文本；返回 true 表示 Chat 合并了工具调用后的文本。
    pub(super) fn text(&mut self, role: Role, text: &str) -> Result<bool> {
        match self {
            Self::Chat(items) => {
                if role == Role::Assistant
                    && let Some(c::Message::Assistant {
                        content,
                        tool_calls: Some(_),
                        ..
                    }) = items.last_mut()
                {
                    let previous = match content {
                        O::Value(c::Content::Text(text)) => text.as_str(),
                        _ => "",
                    };
                    *content = O::Value(c::Content::Text(if previous.is_empty() {
                        text.into()
                    } else {
                        format!("{previous}\n{text}")
                    }));
                    return Ok(true);
                }
                items.push(match role {
                    Role::System => c::Message::System(c::ContentMessage {
                        content: c::Content::Text(text.into()),
                        name: None,
                        extra: Default::default(),
                    }),
                    Role::Developer => c::Message::Developer(c::ContentMessage {
                        content: c::Content::Text(text.into()),
                        name: None,
                        extra: Default::default(),
                    }),
                    Role::User => c::Message::User(c::ContentMessage {
                        content: c::Content::Text(text.into()),
                        name: None,
                        extra: Default::default(),
                    }),
                    Role::Assistant => assistant(O::Value(c::Content::Text(text.into())), None),
                    _ => return Err(unsupported("role", "文本角色无法转换")),
                });
            }
            Self::Responses(items) => items.push(rb::InputItem::Message(rm::Message::Easy(
                rm::EasyInputMessage {
                    content: rm::Content::Text(text.into()),
                    role: match role {
                        Role::User => rm::Role::User,
                        Role::Assistant => rm::Role::Assistant,
                        Role::System => rm::Role::System,
                        Role::Developer => rm::Role::Developer,
                        _ => return Err(unsupported("role", "文本角色无法转换")),
                    },
                    phase: O::Missing,
                    r#type: None,
                    extra: Default::default(),
                },
            ))),
            Self::Messages(items) => {
                let role = match role {
                    Role::User => m::Role::User,
                    Role::Assistant => m::Role::Assistant,
                    _ => return Err(unsupported("role", "文本角色无法转换")),
                };
                if role == m::Role::Assistant && items.last().is_some_and(|item| item.role == role)
                {
                    push_message_block(items, role, text_block(text.into()));
                } else {
                    items.push(m::Message {
                        role,
                        content: m::Content::Text(text.into()),
                        extra: Default::default(),
                    });
                }
            }
            Self::Gemini(items) => {
                let role = match role {
                    Role::User => Some(g::Role::User),
                    Role::Assistant => Some(g::Role::Model),
                    Role::Unspecified => None,
                    _ => return Err(unsupported("role", "文本角色无法转换")),
                };
                let part = g::Part {
                    text: O::Value(text.into()),
                    ..Default::default()
                };
                if role == Some(g::Role::Model) {
                    push_gemini_part(items, g::Role::Model, part);
                } else {
                    items.push(g::Message {
                        role,
                        parts: vec![part],
                        extra: Default::default(),
                    });
                }
            }
        }
        Ok(false)
    }
    /// 追加一个客户端工具调用，同角色的调用并入同一条消息。
    pub(super) fn call(&mut self, id: String, name: &str, args: Map<String, Value>) -> Result<()> {
        match self {
            Self::Chat(items) => {
                let call = c::ToolCall::Function {
                    id,
                    function: c::FunctionCall {
                        name: name.into(),
                        arguments: serde_json::to_string(&args)?,
                        extra: Default::default(),
                    },
                    extra: Default::default(),
                };
                if let Some(c::Message::Assistant { tool_calls, .. }) = items.last_mut() {
                    tool_calls.get_or_insert_with(Vec::new).push(call);
                } else {
                    items.push(assistant(O::Null, Some(vec![call])));
                }
            }
            Self::Responses(items) => items.push(rb::InputItem::FunctionCall(rf::Call {
                r#type: rf::CallType::FunctionCall,
                call_id: O::Value(id),
                name: name.into(),
                arguments: serde_json::to_string(&args)?,
                id: O::Missing,
                status: O::Missing,
                extra: Default::default(),
            })),
            Self::Messages(items) => push_message_block(
                items,
                m::Role::Assistant,
                m::ContentBlock::Known(m::KnownContentBlock::ToolUse {
                    id,
                    name: name.into(),
                    input: args,
                    cache_control: O::Missing,
                    caller: O::Missing,
                    toolset_name: O::Missing,
                    extra: Default::default(),
                }),
            ),
            Self::Gemini(items) => push_gemini_part(
                items,
                g::Role::Model,
                g::Part {
                    function_call: O::Value(g::FunctionCall {
                        id: O::Value(id),
                        name: name.into(),
                        args: O::Value(args),
                        extra: Default::default(),
                    }),
                    ..Default::default()
                },
            ),
        }
        Ok(())
    }
    /// 追加配对后的工具结果；参数中的动态对象不作为协议正文载体。
    pub(super) fn result(
        &mut self,
        id: String,
        name: String,
        content: String,
        response: Map<String, Value>,
    ) {
        match self {
            Self::Chat(items) => items.push(c::Message::Tool {
                tool_call_id: id,
                content: c::Content::Text(content),
                extra: Default::default(),
            }),
            Self::Responses(items) => {
                items.push(rb::InputItem::FunctionCallOutput(rf::CallOutput {
                    r#type: rf::ResultType::FunctionCallOutput,
                    call_id: O::Value(id),
                    output: Value::String(content),
                    id: O::Missing,
                    status: O::Missing,
                    extra: Default::default(),
                }))
            }
            Self::Messages(items) => push_message_block(
                items,
                m::Role::User,
                m::ContentBlock::Known(m::KnownContentBlock::ToolResult {
                    tool_use_id: id,
                    content: O::Value(m::ToolResultContent::Text(content)),
                    cache_control: O::Missing,
                    is_error: O::Missing,
                    toolset_name: O::Missing,
                    extra: Default::default(),
                }),
            ),
            Self::Gemini(items) => push_gemini_part(
                items,
                g::Role::User,
                g::Part {
                    function_response: O::Value(g::FunctionResponse {
                        id: O::Value(id),
                        name,
                        response,
                        parts: O::Missing,
                        will_continue: O::Missing,
                        scheduling: O::Missing,
                        extra: Default::default(),
                    }),
                    ..Default::default()
                },
            ),
        }
    }
    /// 把类型化输入项装入完整目标请求，生成参数和工具由各自模块写入。
    pub(super) fn finish(self, model: &str, system: Vec<String>) -> protocol::Request {
        let system = (!system.is_empty()).then(|| system.join("\n"));
        match self {
            Self::Chat(messages) => protocol::Request::Chat(Box::new(cb::Request {
                model: model.into(),
                messages,
                ..Default::default()
            })),
            Self::Responses(items) => protocol::Request::Responses(Box::new(rb::Request {
                model: O::Value(model.into()),
                input: O::Value(rb::Input::Items(items)),
                instructions: system.into(),
                ..Default::default()
            })),
            Self::Messages(messages) => protocol::Request::Messages(Box::new(mb::Request {
                model: model.into(),
                messages,
                system: system.map(mb::SystemPrompt::Text).into(),
                ..Default::default()
            })),
            Self::Gemini(contents) => protocol::Request::Gemini(Box::new(gb::Request {
                contents,
                system_instruction: system
                    .map(|text| g::Message {
                        role: None,
                        parts: vec![g::Part {
                            text: O::Value(text),
                            ..Default::default()
                        }],
                        extra: Default::default(),
                    })
                    .into(),
                ..Default::default()
            })),
        }
    }
}
/// Chat assistant 的可选字段保持缺失。
fn assistant(
    content: O<c::Content<c::AssistantPart>>,
    tool_calls: Option<Vec<c::ToolCall>>,
) -> c::Message {
    c::Message::Assistant {
        audio: O::Missing,
        content,
        function_call: O::Missing,
        name: None,
        refusal: O::Missing,
        tool_calls,
        extra: Default::default(),
    }
}
/// 创建不含扩展元数据的文本块。
fn text_block(text: String) -> m::ContentBlock {
    m::ContentBlock::Known(m::KnownContentBlock::Text {
        text,
        cache_control: O::Missing,
        citations: O::Missing,
        extra: Default::default(),
    })
}
/// Messages 合并相邻同角色内容，必要时将文本转换成块数组。
fn push_message_block(items: &mut Vec<m::Message>, role: m::Role, block: m::ContentBlock) {
    if let Some(last) = items.last_mut().filter(|last| last.role == role) {
        match &mut last.content {
            m::Content::Parts(parts) => parts.push(block),
            m::Content::Text(text) => {
                last.content = m::Content::Parts(vec![text_block(std::mem::take(text)), block]);
            }
        }
    } else {
        items.push(m::Message {
            role,
            content: m::Content::Parts(vec![block]),
            extra: Default::default(),
        });
    }
}
/// Gemini 合并相邻同角色的内容片段。
fn push_gemini_part(items: &mut Vec<g::Message>, role: g::Role, part: g::Part) {
    if let Some(last) = items.last_mut().filter(|last| last.role == Some(role)) {
        last.parts.push(part);
    } else {
        items.push(g::Message {
            role: Some(role),
            parts: vec![part],
            extra: Default::default(),
        });
    }
}
