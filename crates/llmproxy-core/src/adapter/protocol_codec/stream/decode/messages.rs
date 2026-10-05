//! Messages 明确标识块边界；先合并原始累计用量，再归一化缓存与输入总数。
//! 参考：https://platform.claude.com/docs/en/build-with-claude/streaming

use super::Context;
use crate::{
    adapter::protocol_codec::projection::finish,
    adapter::{Error, Result, response::decode_messages_usage},
    ir::{
        response::{Failure, FinishReason, Status},
        stream::{Event, Head, Key, Metadata, ToolHead},
    },
    protocol::{
        Protocol,
        messages::response::{
            event::{self, Delta, KnownDelta, KnownEvent},
            message::{ContentBlock, KnownContentBlock},
            usage::Usage,
        },
        stream::Event as Raw,
    },
};

/// 原始用量和原生块的索引属于单次消息，不能跨响应共享。
#[derive(Default)]
pub(super) struct Decoder {
    usage: Option<Box<Usage>>,
    reason: Option<FinishReason>,
}

impl Decoder {
    /// 不把 message_delta 的缺失输入误当成零，也不与起始统计相加。
    pub(super) fn decode(&mut self, raw: &event::Event, context: &mut Context<'_>) -> Result<()> {
        let event::Event::Known(event) = raw else {
            let event::Event::Other(event) = raw else {
                unreachable!()
            };
            return context.emit(Event::Unknown {
                protocol: Protocol::AnthropicMessages,
                event: event.clone(),
            });
        };
        match event.as_ref() {
            KnownEvent::MessageStart(start) => {
                if self.usage.is_some() {
                    return Err(Error::Invalid("Messages 重复 message_start".into()));
                }
                context.start(Metadata {
                    id: Some(start.message.id.clone()),
                    model: Some(start.message.model.clone()),
                    created_at: None,
                })?;
                context.candidate(0)?;
                self.usage = Some(Box::new(start.message.usage.clone()));
                context.emit(Event::Usage(
                    decode_messages_usage(&start.message.usage).into(),
                ))?;
                for (index, block) in start.message.content.iter().enumerate() {
                    self.start_block(index as u64, block, context)?;
                }
            }
            KnownEvent::ContentBlockStart(start) => {
                self.start_block(start.index, &start.content_block, context)?
            }
            KnownEvent::ContentBlockDelta(delta) if !native(context, delta.index) => {
                let key = key(delta.index);
                if !context.state.active_keys(0).any(|active| active == key) {
                    return Err(Error::Invalid("Messages 增量没有活动内容块".into()));
                }
                let Delta::Known(delta) = &delta.delta else {
                    return context.emit(Event::Native(Box::new(Raw::Messages(Box::new(
                        raw.clone(),
                    )))));
                };
                match delta.as_ref() {
                    KnownDelta::TextDelta { text, .. } => {
                        require_head(context, key, &Head::Text)?;
                        context.emit(Event::TextDelta {
                            key,
                            text: text.clone(),
                        })?;
                    }
                    KnownDelta::ThinkingDelta { thinking, .. } => {
                        require_head(context, key, &Head::Reasoning)?;
                        context.emit(Event::TextDelta {
                            key,
                            text: thinking.clone(),
                        })?;
                    }
                    KnownDelta::InputJsonDelta { partial_json, .. } => {
                        context.emit(Event::ToolDelta {
                            key,
                            id: None,
                            name: None,
                            arguments: Some(partial_json.clone()),
                        })?;
                    }
                    KnownDelta::SignatureDelta { signature, .. } => {
                        context.emit(Event::Signature {
                            key,
                            protocol: Protocol::AnthropicMessages,
                            data: signature.clone(),
                        })?
                    }
                    KnownDelta::CitationsDelta { citation, .. } => {
                        require_head(context, key, &Head::Text)?;
                        context.emit(Event::Annotation {
                            key,
                            annotation: citation.clone(),
                        })?;
                    }
                }
            }
            KnownEvent::ContentBlockStop(stop) => {
                let key = key(stop.index);
                if native(context, stop.index) {
                    context.emit(Event::Native(Box::new(Raw::Messages(Box::new(
                        raw.clone(),
                    )))))?;
                } else if context.state.part(key).is_some_and(|part| {
                    matches!(part.head, Head::Tool(_)) && part.arguments.is_empty()
                }) {
                    context.emit(Event::ToolDelta {
                        key,
                        id: None,
                        name: None,
                        arguments: Some("{}".into()),
                    })?;
                }
                context.emit(Event::PartEnd(key))?;
            }
            KnownEvent::MessageDelta(delta) => {
                let usage = self
                    .usage
                    .as_mut()
                    .ok_or_else(|| Error::Invalid("Messages 尚未开始".into()))?;
                if let Some(update) = delta.usage.as_option() {
                    if let Some(input) = update.input_tokens.as_option() {
                        usage.input_tokens = *input;
                    }
                    if let Some(output) = update.output_tokens.as_option() {
                        usage.output_tokens = *output;
                    }
                    macro_rules! copy {
                        ($($field:ident),+ $(,)?) => {$(
                            if !update.$field.is_missing() { usage.$field.clone_from(&update.$field); }
                        )+};
                    }
                    copy!(
                        cache_creation_input_tokens,
                        cache_read_input_tokens,
                        server_tool_use
                    );
                    // 缓存 TTL 和推理桶也可能只报告其中一项，需保留其余桶。
                    if let Some(update) = update.cache_creation.as_option() {
                        if !matches!(
                            usage.cache_creation,
                            crate::protocol::OptionalNullable::Value(_)
                        ) {
                            usage.cache_creation =
                                crate::protocol::OptionalNullable::Value(Default::default());
                        }
                        if let crate::protocol::OptionalNullable::Value(target) =
                            &mut usage.cache_creation
                        {
                            if !update.ephemeral_5m_input_tokens.is_missing() {
                                target
                                    .ephemeral_5m_input_tokens
                                    .clone_from(&update.ephemeral_5m_input_tokens);
                            }
                            if !update.ephemeral_1h_input_tokens.is_missing() {
                                target
                                    .ephemeral_1h_input_tokens
                                    .clone_from(&update.ephemeral_1h_input_tokens);
                            }
                        }
                    }
                    if let Some(update) = update.output_tokens_details.as_option() {
                        if !matches!(
                            usage.output_tokens_details,
                            crate::protocol::OptionalNullable::Value(_)
                        ) {
                            usage.output_tokens_details =
                                crate::protocol::OptionalNullable::Value(Default::default());
                        }
                        if let crate::protocol::OptionalNullable::Value(target) =
                            &mut usage.output_tokens_details
                            && !update.thinking_tokens.is_missing()
                        {
                            target.thinking_tokens.clone_from(&update.thinking_tokens);
                        }
                    }
                    context.emit(Event::Usage(decode_messages_usage(usage).into()))?;
                }
                if let Some(reason) = delta.delta.stop_reason.as_option() {
                    let reason = finish(Some(reason));
                    if self.reason.replace(reason).is_some() {
                        return Err(Error::Invalid("Messages 重复停止原因".into()));
                    }
                    context.emit(Event::CandidateEnd { index: 0, reason })?;
                }
            }
            KnownEvent::MessageStop(_) => {
                if self.reason.is_none() {
                    return Err(Error::Invalid("Messages 结束事件缺少停止原因".into()));
                }
                context.emit(Event::End(Status::Completed))?;
            }
            KnownEvent::Ping(_) => context.emit(Event::Heartbeat)?,
            KnownEvent::Error(error) => {
                context.start(Metadata::default())?;
                context.emit(Event::Failure(Failure {
                    code: error.error.r#type.clone(),
                    message: error.error.message.clone(),
                }))?;
                context.emit(Event::End(Status::Failed))?;
            }
            KnownEvent::ContentBlockDelta(delta) => {
                if !context
                    .state
                    .active_keys(0)
                    .any(|key| key == super::messages::key(delta.index))
                {
                    return Err(Error::Invalid("Messages 原生内容块不是活动状态".into()));
                }
                context.emit(Event::Native(Box::new(Raw::Messages(Box::new(
                    raw.clone(),
                )))))?;
            }
        }
        Ok(())
    }

    /// 参数起始空对象是占位，不与 input_json_delta 拼成两个 JSON 值。
    fn start_block(
        &mut self,
        index: u64,
        block: &ContentBlock,
        context: &mut Context<'_>,
    ) -> Result<()> {
        let key = key(index);
        if context.state.part(key).is_some() {
            return Err(Error::Invalid("Messages 重复内容块索引".into()));
        }
        match block {
            ContentBlock::Known(KnownContentBlock::Text {
                text, citations, ..
            }) => {
                context.text(key, Head::Text, text)?;
                for citation in citations.as_option().into_iter().flatten() {
                    context.emit(Event::Annotation {
                        key,
                        annotation: citation.clone(),
                    })?;
                }
            }
            ContentBlock::Known(KnownContentBlock::Thinking {
                thinking,
                signature,
                ..
            }) => {
                context.text(key, Head::Reasoning, thinking)?;
                if !signature.is_empty() {
                    context.emit(Event::Signature {
                        key,
                        protocol: Protocol::AnthropicMessages,
                        data: signature.clone(),
                    })?;
                }
            }
            ContentBlock::Known(KnownContentBlock::ToolUse {
                id, name, input, ..
            }) => {
                context.emit(Event::PartStart {
                    key,
                    head: Head::Tool(ToolHead {
                        id: Some(id.clone()),
                        name: Some(name.clone()),
                        text_input: false,
                    }),
                })?;
                if !input.is_empty() {
                    context.emit(Event::ToolDelta {
                        key,
                        id: None,
                        name: None,
                        arguments: Some(serde_json::to_string(input)?),
                    })?;
                }
            }
            _ => {
                // 原生服务端块需要自己的增量语义，不能伪装成客户端函数。
                context.emit(Event::PartStart {
                    key,
                    head: Head::Native(Protocol::AnthropicMessages),
                })?;
                context.emit(Event::Native(Box::new(Raw::Messages(Box::new(
                    event::Event::Known(Box::new(KnownEvent::ContentBlockStart(
                        event::ContentBlockStart {
                            index,
                            content_block: block.clone(),
                            extra: Default::default(),
                        },
                    ))),
                )))))?;
            }
        }
        Ok(())
    }
}

/// Messages 的内容块索引落在同一候选、同一输出项中。
fn key(index: u64) -> Key {
    Key {
        candidate: 0,
        item: 0,
        part: index,
    }
}

/// 原生块同样进入 IR 状态检查和索引预算。
fn native(context: &Context<'_>, index: u64) -> bool {
    context
        .state
        .part(key(index))
        .is_some_and(|part| matches!(part.head, Head::Native(_)))
}

/// 文本/思考事件不能写入工具块，工具参数也不能写入文本块。
fn require_head(context: &Context<'_>, key: Key, expected: &Head) -> Result<()> {
    if !context
        .state
        .part(key)
        .is_some_and(|part| part.head == *expected)
    {
        return Err(Error::Invalid("Messages 增量与内容块类型不匹配".into()));
    }
    Ok(())
}
