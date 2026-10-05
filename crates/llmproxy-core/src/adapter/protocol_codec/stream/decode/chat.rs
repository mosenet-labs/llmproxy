//! Chat 候选的文本及工具参数都是追加片段；用量通常在空 choices 分片中报告。
//! 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events

use super::Context;
use crate::{
    adapter::protocol_codec::projection::finish,
    adapter::{Error, Result, response::decode_chat_usage},
    ir::stream::{Event, Head, Key, Metadata, ToolHead},
    protocol::{
        OptionalNullable as O,
        chat::response::{
            Chunk,
            chunk::{Choice, Delta},
        },
        stream::Event as Raw,
    },
};
use std::collections::BTreeMap;

/// 只记录音频是否已收到结束元数据，不累计音频或转录正文。
#[derive(Default)]
pub(super) struct Decoder {
    audio: BTreeMap<u64, bool>,
}

impl Decoder {
    /// 一个 Chunk 可以同时更新多个候选和多个并行工具。
    pub(super) fn decode(&mut self, chunk: &Chunk, context: &mut Context<'_>) -> Result<()> {
        if chunk.object != "chat.completion.chunk" {
            return Err(Error::Invalid("不是 Chat 流式分片".into()));
        }
        if context
            .state
            .metadata()
            .is_some_and(|head| head.id.as_deref() != Some(chunk.id.as_str()))
        {
            return Err(Error::Invalid("Chat 流式响应 ID 改变".into()));
        }
        context.start(Metadata {
            id: Some(chunk.id.clone()),
            model: Some(chunk.model.clone()),
            created_at: Some(chunk.created),
        })?;
        for choice in &chunk.choices {
            let index = choice.index;
            if context
                .state
                .candidate(index)
                .is_some_and(|candidate| candidate.is_some())
            {
                let delta = &choice.delta;
                if delta.content.as_option().is_some()
                    || delta.refusal.as_option().is_some()
                    || delta.role.as_option().is_some()
                    || delta.tool_calls.as_option().is_some()
                    || delta.function_call.as_option().is_some()
                    || choice.finish_reason.as_option().is_some()
                    || !delta.audio.as_option().is_some_and(|audio| {
                        audio.expires_at.as_option().is_some()
                            && audio.id.as_option().is_none()
                            && audio.data.as_option().is_none()
                            && audio.transcript.as_option().is_none()
                    })
                {
                    return Err(Error::Invalid(
                        "Chat 已结束候选仅允许音频到期元数据尾帧".into(),
                    ));
                }
                self.audio(chunk, choice, context)?;
                continue;
            }
            context.candidate(index)?;
            let delta = &choice.delta;
            if delta
                .role
                .as_option()
                .is_some_and(|role| role != "assistant")
            {
                return Err(Error::Invalid("Chat 回复增量角色不是 assistant".into()));
            }
            if let Some(text) = delta.content.as_option() {
                context.text(
                    Key {
                        candidate: index,
                        item: 0,
                        part: 0,
                    },
                    Head::Text,
                    text,
                )?;
            }
            if let Some(text) = delta.refusal.as_option() {
                context.text(
                    Key {
                        candidate: index,
                        item: 0,
                        part: 1,
                    },
                    Head::Refusal,
                    text,
                )?;
            }
            if let Some(calls) = delta.tool_calls.as_option() {
                if delta.function_call.as_option().is_some() {
                    return Err(Error::Invalid("Chat 增量同时包含两种函数调用格式".into()));
                }
                for call in calls {
                    if call
                        .r#type
                        .as_option()
                        .is_some_and(|kind| kind != "function")
                    {
                        return Err(Error::Unsupported("Chat 流式工具类型尚无事件映射".into()));
                    }
                    let item = call
                        .index
                        .checked_add(1)
                        .ok_or_else(|| Error::Invalid("工具索引溢出".into()))?;
                    let key = Key {
                        candidate: index,
                        item,
                        part: 0,
                    };
                    context.part(key, Head::Tool(ToolHead::default()))?;
                    context.emit(Event::ToolDelta {
                        key,
                        id: call.id.as_option().cloned(),
                        name: call
                            .function
                            .as_option()
                            .and_then(|f| f.name.as_option())
                            .cloned(),
                        arguments: call
                            .function
                            .as_option()
                            .and_then(|f| f.arguments.as_option())
                            .cloned(),
                    })?;
                }
            }
            if let Some(call) = delta.function_call.as_option() {
                let key = Key {
                    candidate: index,
                    item: 1,
                    part: 0,
                };
                context.part(key, Head::Tool(ToolHead::default()))?;
                context.emit(Event::ToolDelta {
                    key,
                    id: None,
                    name: call.name.as_option().cloned(),
                    arguments: call.arguments.as_option().cloned(),
                })?;
            }
            if delta.audio.as_option().is_some() {
                self.audio(chunk, choice, context)?;
            }
            if let Some(reason) = choice.finish_reason.as_option() {
                context.close_candidate(index, finish(Some(reason)))?;
            }
        }
        if let Some(usage) = chunk.usage.as_option() {
            context.emit(Event::Usage(decode_chat_usage(usage).into()))?;
        }
        Ok(())
    }

    /// 原生载体仅包含音频，已投影的文本、工具及用量不重复回放。
    fn audio(&mut self, chunk: &Chunk, choice: &Choice, context: &mut Context<'_>) -> Result<()> {
        let audio = choice
            .delta
            .audio
            .as_option()
            .ok_or_else(|| Error::Invalid("音频尾帧缺少音频字段".into()))?;
        if self.audio.get(&choice.index) == Some(&true) {
            return Err(Error::Invalid("Chat 音频结束后仍有增量".into()));
        }
        if !self.audio.contains_key(&choice.index)
            && audio.data.as_option().is_none()
            && audio.transcript.as_option().is_none()
            && audio.id.as_option().is_none()
        {
            return Err(Error::Invalid("Chat 音频到期更新早于音频开始".into()));
        }
        self.audio
            .insert(choice.index, audio.expires_at.as_option().is_some());
        context.emit(Event::Native(Box::new(Raw::Chat(Box::new(Chunk {
            id: chunk.id.clone(),
            created: chunk.created,
            model: chunk.model.clone(),
            object: chunk.object.clone(),
            choices: vec![Choice {
                index: choice.index,
                finish_reason: O::Missing,
                logprobs: O::Missing,
                extra: Default::default(),
                delta: Delta {
                    audio: O::Value(audio.clone()),
                    content: O::Missing,
                    refusal: O::Missing,
                    role: O::Missing,
                    tool_calls: O::Missing,
                    function_call: O::Missing,
                    extra: Default::default(),
                },
            }],
            moderation: O::Missing,
            obfuscation: O::Missing,
            service_tier: O::Missing,
            system_fingerprint: O::Missing,
            usage: O::Missing,
            extra: Default::default(),
        })))))
    }

    /// 音频未收到 expires_at 的 EOF 不能当成正常完成。
    pub(super) fn end(&self) -> Result<()> {
        if self.audio.values().any(|done| !done) {
            return Err(Error::Invalid("Chat 音频缺少结束元数据".into()));
        }
        Ok(())
    }
}
