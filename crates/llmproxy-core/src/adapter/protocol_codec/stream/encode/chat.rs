//! IR 文本和参数增量构造 Chat 分片；最终用量独立于候选结束。
//! 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events
use std::collections::BTreeMap;

use super::{Context, finish, unsupported};
use crate::{
    adapter::{Error, Result, response::encode_chat_usage},
    ir::{
        response::Status,
        stream::{Event, Head, Key},
    },
    protocol::{
        OptionalNullable as O, Protocol,
        chat::response::chunk::{Choice, Chunk, Delta, FunctionCallDelta, ToolCallDelta},
        stream::Event as Raw,
    },
};

/// 仅保留工具的目标索引，正文字符串不进入 Chat 目标缓冲。
#[derive(Default)]
pub(super) struct Encoder {
    tools: BTreeMap<Key, Tool>,
}

/// 首个有效增量之前不补造 ID；已发送的 ID 不能被晚到的来源 ID 覆盖。
struct Tool {
    index: u64,
    started: bool,
    generated_id: bool,
}

impl Encoder {
    /// 音频原生增量使用目标外壳；正文等通用字段由独立 IR 事件发送。
    pub(super) fn audio(
        &mut self,
        index: u64,
        audio: &crate::protocol::chat::response::chunk::AudioDelta,
        context: &mut Context<'_>,
    ) {
        let mut audio = audio.clone();
        audio.extra.clear();
        choice(
            context,
            index,
            Delta {
                audio: O::Value(audio),
                ..empty_delta()
            },
            O::Missing,
        );
    }
    /// 一次 IR 事件可生成首块、正文、停止原因或最终用量分片。
    pub(super) fn encode(&mut self, event: &Event, context: &mut Context<'_>) -> Result<()> {
        match event {
            Event::CandidateStart(index) => choice(
                context,
                *index,
                Delta {
                    role: O::Value("assistant".into()),
                    ..empty_delta()
                },
                O::Null,
            ),
            Event::PartStart {
                key,
                head: Head::Tool(head),
            } => {
                if head.text_input {
                    return Err(unsupported(
                        "tool.input",
                        "Chat 函数工具不能表达自由文本输入",
                    ));
                }
                let index = self
                    .tools
                    .keys()
                    .filter(|part| part.candidate == key.candidate)
                    .count() as u64;
                let started = head.id.is_some() || head.name.is_some();
                let generated_id = head.id.as_ref().is_none_or(String::is_empty);
                self.tools.insert(
                    *key,
                    Tool {
                        index,
                        started,
                        generated_id,
                    },
                );
                if started {
                    choice(
                        context,
                        key.candidate,
                        tool(
                            index,
                            Some(context.call_id(*key, head)),
                            head.name.clone(),
                            None,
                        ),
                        O::Null,
                    );
                }
            }
            Event::PartStart {
                head: Head::Reasoning,
                ..
            } => context.warn("reasoning", "思考正文写入兼容扩展 reasoning_content"),
            Event::TextDelta { key, text } => {
                let delta = match &context.part(*key)?.head {
                    Head::Text => Delta {
                        content: O::Value(text.clone()),
                        ..empty_delta()
                    },
                    Head::Refusal => Delta {
                        refusal: O::Value(text.clone()),
                        ..empty_delta()
                    },
                    // 与非流式 Chat 使用相同兼容字段，逐段交付可见思考而不混入回答正文。
                    Head::Reasoning => Delta {
                        extra: [("reasoning_content".into(), text.clone().into())]
                            .into_iter()
                            .collect(),
                        ..empty_delta()
                    },
                    _ => return Err(Error::Invalid("Chat 文本增量的块类型错误".into())),
                };
                choice(context, key.candidate, delta, O::Null);
            }
            Event::ToolDelta {
                key,
                id,
                name,
                arguments,
            } => {
                let current = self
                    .tools
                    .get_mut(key)
                    .ok_or_else(|| Error::Invalid("Chat 工具索引不存在".into()))?;
                if !current.started && (id.is_some() || name.is_some() || arguments.is_some()) {
                    let Head::Tool(head) = &context.part(*key)?.head else {
                        return Err(Error::Invalid("Chat 工具块类型错误".into()));
                    };
                    current.generated_id = head.id.as_ref().is_none_or(String::is_empty);
                    current.started = true;
                    choice(
                        context,
                        key.candidate,
                        tool(
                            current.index,
                            Some(context.call_id(*key, head)),
                            name.clone(),
                            arguments.clone(),
                        ),
                        O::Null,
                    );
                    return Ok(());
                }
                if id.is_some() && current.generated_id {
                    context.warn(
                        "tool.id",
                        "目标工具 ID 已在开始时固定，后续来源 ID 由接入方关联",
                    );
                }
                if name.is_some() || arguments.is_some() {
                    choice(
                        context,
                        key.candidate,
                        tool(current.index, None, name.clone(), arguments.clone()),
                        O::Null,
                    );
                }
            }
            Event::CandidateEnd { index, reason } => choice(
                context,
                *index,
                empty_delta(),
                O::Value(finish(Protocol::OpenAiChat, *reason)?.into()),
            ),
            Event::End(Status::Completed | Status::Incomplete) => {
                if let Some(usage) = context.usage()? {
                    let mut chunk = empty_chunk(context);
                    chunk.usage = O::Value(encode_chat_usage(&usage, None)?);
                    context.events.push(Raw::Chat(Box::new(chunk)));
                }
                context.events.push(Raw::End(Protocol::OpenAiChat));
            }
            Event::Failure(_) | Event::End(Status::Failed | Status::Cancelled) => {
                return Err(Error::FailedResponse);
            }
            Event::Media { .. } => {
                return Err(unsupported("media", "Chat 输出没有等价的完整媒体内容块"));
            }
            Event::Signature { .. } => context.warn(
                "signature",
                "Chat 不能表达来源签名，需由接入方保存工具回合状态",
            ),
            Event::Annotation { .. } => {
                context.warn("annotations", "Chat 标准增量没有等价引用事件，已丢弃")
            }
            _ => {}
        }
        Ok(())
    }
}

/// 所有外壳值来自目标配置，不能复制来源模型或对象类型。
fn empty_chunk(context: &Context<'_>) -> Chunk {
    Chunk {
        id: context.target.id.clone(),
        choices: Vec::new(),
        created: context.target.created,
        model: context.target.model.clone(),
        object: "chat.completion.chunk".into(),
        moderation: O::Missing,
        obfuscation: O::Missing,
        service_tier: O::Missing,
        system_fingerprint: O::Missing,
        usage: O::Missing,
        extra: Default::default(),
    }
}

/// 缺失的字段保持缺失，不把空 delta 误写成 null 正文。
fn empty_delta() -> Delta {
    Delta {
        audio: O::Missing,
        content: O::Missing,
        function_call: O::Missing,
        refusal: O::Missing,
        role: O::Missing,
        tool_calls: O::Missing,
        extra: Default::default(),
    }
}

/// 单候选变化独立发送，以支持交错到达的多个候选。
fn choice(context: &mut Context<'_>, index: u64, delta: Delta, finish_reason: O<String>) {
    let mut chunk = empty_chunk(context);
    chunk.choices.push(Choice {
        delta,
        finish_reason,
        index,
        logprobs: O::Missing,
        extra: Default::default(),
    });
    context.events.push(Raw::Chat(Box::new(chunk)));
}

/// 只构造函数参数字符串叶子，不尝试解码尚未闭合的 JSON。
fn tool(index: u64, id: Option<String>, name: Option<String>, arguments: Option<String>) -> Delta {
    Delta {
        tool_calls: O::Value(vec![ToolCallDelta {
            index,
            id: id.into(),
            r#type: O::Value("function".into()),
            function: O::Value(FunctionCallDelta {
                name: name.into(),
                arguments: arguments.into(),
                extra: Default::default(),
            }),
            extra: Default::default(),
        }]),
        ..empty_delta()
    }
}
