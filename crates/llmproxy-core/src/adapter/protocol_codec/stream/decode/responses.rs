//! Responses 同时发送增量和完成快照；快照用于校验，不重复交付正文。
//! 参考：https://developers.openai.com/api/reference/resources/responses/streaming-events

use super::Context;
use crate::{
    adapter::{Error, Result, response::decode_responses_usage},
    ir::{
        response::{Failure, FinishReason, Status},
        stream::{Event, Head, Key, Metadata, ToolHead},
    },
    protocol::{
        Protocol,
        responses::response::{
            body::{OutputItem, Response},
            event::{self, ContentPart, KnownEvent, ReasoningPart},
            message::ContentPart as OutputPart,
        },
        stream::Event as Raw,
    },
};
use std::collections::BTreeMap;

/// 完成标记和文本字节计数足够去重，不需要保存整个文本响应。
#[derive(Default)]
pub(super) struct Decoder {
    sequence: Option<u64>,
    items: BTreeMap<u64, bool>,
    text_bytes: BTreeMap<Key, usize>,
    annotations: BTreeMap<Key, usize>,
    had_tool: bool,
    /// 音频和转录各有独立 done；正文不进入累计缓冲。
    audio: Option<bool>,
    transcript: Option<bool>,
}

impl Decoder {
    /// 生命周期终止才结束整条流，内容 done 仅结束该内容块。
    pub(super) fn decode(&mut self, raw: &event::Event, context: &mut Context<'_>) -> Result<()> {
        let event::Event::Known(event) = raw else {
            let event::Event::Other(event) = raw else {
                unreachable!()
            };
            return context.emit(Event::Unknown {
                protocol: Protocol::OpenAiResponses,
                event: event.clone(),
            });
        };
        let sequence = event.sequence_number();
        if self.sequence.is_some_and(|old| sequence <= old) {
            return Err(Error::Invalid("Responses 事件顺序号重复或倒退".into()));
        }
        self.sequence = Some(sequence);
        if context.state.metadata().is_none()
            && !matches!(
                event.as_ref(),
                KnownEvent::Created(_)
                    | KnownEvent::InProgress(_)
                    | KnownEvent::Queued(_)
                    | KnownEvent::Completed(_)
                    | KnownEvent::Incomplete(_)
                    | KnownEvent::Failed(_)
                    | KnownEvent::Error(_)
            )
        {
            return Err(Error::Invalid("Responses 内容事件早于生命周期开始".into()));
        }
        match event.as_ref() {
            KnownEvent::Created(lifecycle)
            | KnownEvent::InProgress(lifecycle)
            | KnownEvent::Queued(lifecycle) => self.start(&lifecycle.response, context)?,
            KnownEvent::Completed(lifecycle) | KnownEvent::Incomplete(lifecycle) => {
                if self.audio == Some(false) || self.transcript == Some(false) {
                    return Err(Error::Invalid("Responses 音频或转录缺少 done 事件".into()));
                }
                self.start(&lifecycle.response, context)?;
                for (index, item) in lifecycle.response.output.iter().enumerate() {
                    self.item_done(index as u64, item, context)?;
                }
                if let Some(usage) = lifecycle.response.usage.as_option() {
                    context.emit(Event::Usage(decode_responses_usage(usage).into()))?;
                }
                let reason = match lifecycle
                    .response
                    .incomplete_details
                    .as_option()
                    .map(|details| details.reason.as_str())
                {
                    Some("max_output_tokens") => FinishReason::Length,
                    Some("content_filter") => FinishReason::Filtered,
                    Some("refusal") => FinishReason::Refusal,
                    _ if matches!(event.as_ref(), KnownEvent::Incomplete(_)) => {
                        FinishReason::Unknown
                    }
                    _ if self.had_tool => FinishReason::ToolCall,
                    _ => FinishReason::Stop,
                };
                context.close_candidate(0, reason)?;
                context.emit(Event::End(
                    if matches!(event.as_ref(), KnownEvent::Incomplete(_)) {
                        Status::Incomplete
                    } else {
                        Status::Completed
                    },
                ))?;
            }
            KnownEvent::Failed(lifecycle) => {
                self.start(&lifecycle.response, context)?;
                if let Some(usage) = lifecycle.response.usage.as_option() {
                    context.emit(Event::Usage(decode_responses_usage(usage).into()))?;
                }
                let failure = lifecycle
                    .response
                    .error
                    .as_option()
                    .map(|error| Failure {
                        code: error.code.clone(),
                        message: error.message.clone(),
                    })
                    .unwrap_or(Failure {
                        code: "generation_failed".into(),
                        message: String::new(),
                    });
                context.emit(Event::Failure(failure))?;
                context.emit(Event::End(Status::Failed))?;
            }
            KnownEvent::OutputItemAdded(item) => {
                self.item_start(item.output_index, &item.item, context)?
            }
            KnownEvent::OutputItemDone(item) => {
                self.item_done(item.output_index, &item.item, context)?
            }
            KnownEvent::ContentPartAdded(part) => {
                let key = key(part.output_index, part.content_index);
                if context.state.part(key).is_some() {
                    return Err(Error::Invalid("Responses 重复内容块开始".into()));
                }
                self.content(key, &part.part, false, context)?;
            }
            KnownEvent::ContentPartDone(part) => {
                let key = key(part.output_index, part.content_index);
                self.content(key, &part.part, true, context)?;
                end_part(key, context)?;
            }
            KnownEvent::OutputTextDelta(delta)
            | KnownEvent::RefusalDelta(delta)
            | KnownEvent::ReasoningTextDelta(delta) => {
                let head = match event.as_ref() {
                    KnownEvent::RefusalDelta(_) => Head::Refusal,
                    KnownEvent::ReasoningTextDelta(_) => Head::Reasoning,
                    _ => Head::Text,
                };
                self.delta(
                    key(delta.output_index, delta.content_index),
                    head,
                    &delta.delta,
                    context,
                )?;
            }
            KnownEvent::OutputTextDone(done) | KnownEvent::ReasoningTextDone(done) => {
                self.snapshot(
                    key(done.output_index, done.content_index),
                    if matches!(event.as_ref(), KnownEvent::ReasoningTextDone(_)) {
                        Head::Reasoning
                    } else {
                        Head::Text
                    },
                    &done.text,
                    context,
                )?;
            }
            KnownEvent::RefusalDone(done) => self.snapshot(
                key(done.output_index, done.content_index),
                Head::Refusal,
                &done.refusal,
                context,
            )?,
            KnownEvent::FunctionArgumentsDelta(delta) | KnownEvent::CustomToolInputDelta(delta) => {
                context.emit(Event::ToolDelta {
                    key: key(delta.output_index, 0),
                    id: None,
                    name: None,
                    arguments: Some(delta.delta.clone()),
                })?
            }
            KnownEvent::FunctionArgumentsDone(done) => {
                self.arguments(key(done.output_index, 0), &done.arguments, context)?
            }
            KnownEvent::CustomToolInputDone(done) => {
                self.arguments(key(done.output_index, 0), &done.input, context)?
            }
            KnownEvent::ReasoningSummaryPartAdded(part) => {
                let key = summary_key(part.output_index, part.summary_index)?;
                if context.state.part(key).is_some() {
                    return Err(Error::Invalid("Responses 重复思考摘要块".into()));
                }
                self.delta(key, Head::Reasoning, reasoning_text(&part.part), context)?;
            }
            KnownEvent::ReasoningSummaryPartDone(part) => {
                let key = summary_key(part.output_index, part.summary_index)?;
                self.snapshot(key, Head::Reasoning, reasoning_text(&part.part), context)?;
                end_part(key, context)?;
            }
            KnownEvent::ReasoningSummaryTextDelta(delta) => self.delta(
                summary_key(delta.output_index, delta.summary_index)?,
                Head::Reasoning,
                &delta.delta,
                context,
            )?,
            KnownEvent::ReasoningSummaryTextDone(done) => self.snapshot(
                summary_key(done.output_index, done.summary_index)?,
                Head::Reasoning,
                &done.text,
                context,
            )?,
            KnownEvent::AnnotationAdded(annotation) => {
                let key = key(annotation.output_index, annotation.content_index);
                let count = self.annotations.get(&key).copied().unwrap_or(0);
                let index = usize::try_from(annotation.annotation_index)
                    .map_err(|_| Error::Invalid("引用索引溢出".into()))?;
                if index > count {
                    return Err(Error::Invalid("Responses 引用索引不连续".into()));
                }
                if index == count {
                    context.emit(Event::Annotation {
                        key,
                        annotation: annotation
                            .annotation
                            .as_option()
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    })?;
                    self.annotations.insert(key, count + 1);
                }
            }
            KnownEvent::Error(error) => {
                context.start(Metadata::default())?;
                context.emit(Event::Failure(Failure {
                    code: error
                        .code
                        .as_option()
                        .cloned()
                        .unwrap_or_else(|| "stream_error".into()),
                    message: error.message.clone(),
                }))?;
                context.emit(Event::End(Status::Failed))?;
            }
            KnownEvent::AudioDelta(delta) | KnownEvent::AudioTranscriptDelta(delta) => {
                audio_id(&delta.response_id, context)?;
                let state = if matches!(event.as_ref(), KnownEvent::AudioDelta(_)) {
                    &mut self.audio
                } else {
                    &mut self.transcript
                };
                if *state == Some(true) {
                    return Err(Error::Invalid("Responses 音频 done 后仍有增量".into()));
                }
                *state = Some(false);
                context.emit(Event::Native(Box::new(Raw::Responses(Box::new(
                    raw.clone(),
                )))))?;
            }
            KnownEvent::AudioDone(done) | KnownEvent::AudioTranscriptDone(done) => {
                audio_id(&done.response_id, context)?;
                let state = if matches!(event.as_ref(), KnownEvent::AudioDone(_)) {
                    &mut self.audio
                } else {
                    &mut self.transcript
                };
                if *state != Some(false) {
                    return Err(Error::Invalid("Responses 音频 done 重复或早于增量".into()));
                }
                *state = Some(true);
                context.emit(Event::Native(Box::new(Raw::Responses(Box::new(
                    raw.clone(),
                )))))?;
            }
            // 服务端执行进度、图片预览、音频和 MCP 具有各自语义，留给目标策略。
            _ => {
                let index = native_index(event);
                if !index.is_some_and(|index| {
                    self.items.get(&index) == Some(&false)
                        && context
                            .state
                            .part(key(index, 0))
                            .is_some_and(|part| matches!(part.head, Head::Native(_)) && !part.ended)
                }) {
                    return Err(Error::Invalid(
                        "Responses 原生进度没有对应活动服务端输出项".into(),
                    ));
                }
                context.emit(Event::Native(Box::new(Raw::Responses(Box::new(
                    raw.clone(),
                )))))?;
            }
        }
        Ok(())
    }

    /// 每次生命周期快照必须属于同一个响应。
    fn start(&self, response: &Response, context: &mut Context<'_>) -> Result<()> {
        if context
            .state
            .metadata()
            .is_some_and(|head| head.id.as_deref() != Some(response.id.as_str()))
        {
            return Err(Error::Invalid("Responses 流式响应 ID 改变".into()));
        }
        context.start(Metadata {
            id: Some(response.id.clone()),
            model: Some(response.model.clone()),
            created_at: Some(response.created_at),
        })?;
        context.candidate(0)
    }

    /// 输出项的索引记录也受上限约束，包括没有文本内容的原生项。
    fn register(&mut self, index: u64, context: &Context<'_>) -> Result<()> {
        if self.items.contains_key(&index) {
            return Err(Error::Invalid("Responses 重复输出项".into()));
        }
        if self.items.len() >= context.entries {
            return Err(Error::Invalid("Responses 输出项超过上限".into()));
        }
        self.items.insert(index, false);
        Ok(())
    }

    /// 函数参数起始字符串可能为空，函数调用 ID 与输出项 ID 不混用。
    fn item_start(
        &mut self,
        index: u64,
        item: &OutputItem,
        context: &mut Context<'_>,
    ) -> Result<()> {
        self.register(index, context)?;
        match item {
            OutputItem::FunctionCall(call) => {
                let key = key(index, 0);
                context.emit(Event::PartStart {
                    key,
                    head: Head::Tool(ToolHead {
                        id: call.call_id.as_option().cloned(),
                        name: Some(call.name.clone()),
                        text_input: false,
                    }),
                })?;
                if !call.arguments.is_empty() {
                    context.emit(Event::ToolDelta {
                        key,
                        id: None,
                        name: None,
                        arguments: Some(call.arguments.clone()),
                    })?;
                }
                self.had_tool = true;
            }
            OutputItem::Message(message) => {
                for (part, content) in message.content.iter().enumerate() {
                    self.output(key(index, part as u64), content, false, context)?;
                }
            }
            OutputItem::Other(raw)
                if raw.get("type").and_then(serde_json::Value::as_str) == Some("reasoning") => {}
            OutputItem::Other(raw)
                if raw.get("type").and_then(serde_json::Value::as_str)
                    == Some("custom_tool_call") =>
            {
                let name = raw
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::Invalid("自定义工具没有名称".into()))?;
                context.emit(Event::PartStart {
                    key: key(index, 0),
                    head: Head::Tool(ToolHead {
                        id: raw
                            .get("call_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                        name: Some(name.into()),
                        text_input: true,
                    }),
                })?;
                if let Some(input) = raw
                    .get("input")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    context.emit(Event::ToolDelta {
                        key: key(index, 0),
                        id: None,
                        name: None,
                        arguments: Some(input.into()),
                    })?;
                }
                self.had_tool = true;
            }
            _ => context.emit(Event::PartStart {
                key: key(index, 0),
                head: Head::Native(Protocol::OpenAiResponses),
            })?,
        }
        Ok(())
    }

    /// 完成快照可以补齐从未发送过的内容，但不能把已发送的内容再追加一遍。
    fn item_done(
        &mut self,
        index: u64,
        item: &OutputItem,
        context: &mut Context<'_>,
    ) -> Result<()> {
        if self.items.get(&index) == Some(&true) {
            return Ok(());
        }
        if !self.items.contains_key(&index) {
            self.item_start(index, item, context)?;
        }
        match item {
            OutputItem::FunctionCall(call) => {
                self.arguments(key(index, 0), &call.arguments, context)?
            }
            OutputItem::Message(message) => {
                for (part, content) in message.content.iter().enumerate() {
                    self.output(key(index, part as u64), content, true, context)?;
                }
            }
            OutputItem::Other(raw)
                if raw.get("type").and_then(serde_json::Value::as_str) == Some("reasoning") =>
            {
                for (part, content) in raw
                    .get("summary")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(text) = content.get("text").and_then(serde_json::Value::as_str) {
                        self.snapshot(
                            summary_key(index, part as u64)?,
                            Head::Reasoning,
                            text,
                            context,
                        )?;
                    }
                }
            }
            OutputItem::Other(raw)
                if raw.get("type").and_then(serde_json::Value::as_str)
                    == Some("custom_tool_call") =>
            {
                let input = raw
                    .get("input")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::Invalid("自定义工具没有完整输入".into()))?;
                self.arguments(key(index, 0), input, context)?;
            }
            OutputItem::Other(raw) => {
                if let Some(output) =
                    crate::adapter::server_output::decode(Protocol::OpenAiResponses, raw)
                {
                    context.emit(Event::ServerOutput {
                        key: key(index, u64::MAX),
                        output,
                    })?;
                } else {
                    context.emit(Event::Native(Box::new(Raw::Responses(Box::new(
                        event::Event::Known(Box::new(KnownEvent::OutputItemDone(
                            event::ItemEvent {
                                item: item.clone(),
                                output_index: index,
                                sequence_number: self.sequence.unwrap_or(0),
                                extra: Default::default(),
                            },
                        ))),
                    )))))?;
                }
            }
        }
        let keys = context
            .state
            .active_keys(0)
            .filter(|key| key.item == index)
            .collect::<Vec<_>>();
        for key in keys {
            end_part(key, context)?;
        }
        self.items.insert(index, true);
        Ok(())
    }

    /// 内容块的起始及完成快照共用同一语义字段投影。
    fn content(
        &mut self,
        key: Key,
        part: &ContentPart,
        done: bool,
        context: &mut Context<'_>,
    ) -> Result<()> {
        match part {
            ContentPart::Output(part) => self.output(key, part, done, context),
            ContentPart::Reasoning(part) if done => {
                self.snapshot(key, Head::Reasoning, reasoning_text(part), context)
            }
            ContentPart::Reasoning(part) => {
                self.delta(key, Head::Reasoning, reasoning_text(part), context)
            }
        }
    }

    /// 引用可在后续 annotation 事件中补齐；文本仍保持原始增量顺序。
    fn output(
        &mut self,
        key: Key,
        part: &OutputPart,
        done: bool,
        context: &mut Context<'_>,
    ) -> Result<()> {
        let (head, text) = match part {
            OutputPart::OutputText { text, .. } => (Head::Text, text.as_str()),
            OutputPart::Refusal { refusal, .. } => (Head::Refusal, refusal.as_str()),
        };
        if done {
            self.snapshot(key, head, text, context)?;
        } else {
            self.delta(key, head, text, context)?;
        }
        if let OutputPart::OutputText { annotations, .. } = part {
            let count = self.annotations.get(&key).copied().unwrap_or(0);
            for annotation in annotations.iter().skip(count) {
                context.emit(Event::Annotation {
                    key,
                    annotation: annotation.clone(),
                })?;
            }
            self.annotations.insert(key, count.max(annotations.len()));
        }
        Ok(())
    }

    /// 只累计字节计数，用于检查完成快照是否与增量长度一致。
    fn delta(&mut self, key: Key, head: Head, text: &str, context: &mut Context<'_>) -> Result<()> {
        context.text(key, head, text)?;
        let count = self.text_bytes.entry(key).or_default();
        *count = count
            .checked_add(text.len())
            .ok_or_else(|| Error::Invalid("Responses 文本长度溢出".into()))?;
        Ok(())
    }

    /// 没有任何增量时允许从快照交付正文，收到增量后不得追加全量快照。
    fn snapshot(
        &mut self,
        key: Key,
        head: Head,
        text: &str,
        context: &mut Context<'_>,
    ) -> Result<()> {
        match self.text_bytes.get(&key).copied().unwrap_or(0) {
            0 if !context.state.part(key).is_some_and(|part| part.ended) => {
                self.delta(key, head, text, context)
            }
            count if count == text.len() => Ok(()),
            _ => Err(Error::Invalid("Responses 完成文本与增量长度不一致".into())),
        }
    }

    /// 工具参数本来就受局部缓冲上限约束，可直接与完成参数比较。
    fn arguments(&self, key: Key, full: &str, context: &mut Context<'_>) -> Result<()> {
        let part = context
            .state
            .part(key)
            .ok_or_else(|| Error::Invalid("Responses 参数没有工具项".into()))?;
        if !matches!(part.head, Head::Tool(_)) || part.ended {
            return Err(Error::Invalid("Responses 参数写入非活动工具项".into()));
        }
        if part.arguments.is_empty() {
            context.emit(Event::ToolDelta {
                key,
                id: None,
                name: None,
                arguments: Some(full.into()),
            })
        } else if part.arguments == full {
            Ok(())
        } else {
            Err(Error::Invalid("Responses 完成参数与增量不一致".into()))
        }
    }
}

/// 服务端进度必须归属于输出项，不能混入客户端工具或已经完成的项目。
fn native_index(event: &KnownEvent) -> Option<u64> {
    use KnownEvent::*;
    Some(match event {
        FileSearchInProgress(e)
        | FileSearchSearching(e)
        | FileSearchCompleted(e)
        | WebSearchInProgress(e)
        | WebSearchSearching(e)
        | WebSearchCompleted(e)
        | CodeInterpreterInProgress(e)
        | CodeInterpreterInterpreting(e)
        | CodeInterpreterCompleted(e)
        | McpCallInProgress(e)
        | McpCallCompleted(e)
        | McpCallFailed(e)
        | McpListToolsInProgress(e)
        | McpListToolsCompleted(e)
        | McpListToolsFailed(e)
        | ImageGenerationInProgress(e)
        | ImageGenerationGenerating(e)
        | ImageGenerationCompleted(e)
        | CompactionCompacting(e) => e.output_index,
        McpArgumentsDelta(e) | CodeInterpreterCodeDelta(e) => e.output_index,
        McpArgumentsDone(e) => e.output_index,
        CodeInterpreterCodeDone(e) => e.output_index,
        ImageGenerationPartial(e) => e.output_index,
        ShellCommandAdded(e) | ShellCommandDone(e) => e.output_index,
        ShellCommandDelta(e) => e.output_index,
        ShellOutputDelta(e) => e.output_index,
        ShellOutputDone(e) => e.output_index,
        _ => return None,
    })
}

/// 无 ID 的官方事件也可解码；携带 ID 时必须属于当前生命周期。
fn audio_id(id: &crate::protocol::OptionalNullable<String>, context: &Context<'_>) -> Result<()> {
    if id.as_option().is_some_and(|id| {
        context
            .state
            .metadata()
            .and_then(|metadata| metadata.id.as_ref())
            != Some(id)
    }) {
        return Err(Error::Invalid("Responses 音频响应 ID 不匹配".into()));
    }
    Ok(())
}

/// 普通内容索引使用低半区；思考摘要使用高半区，避免与正文索引混用。
fn summary_key(item: u64, part: u64) -> Result<Key> {
    Ok(key(
        item,
        part.checked_add(1 << 63)
            .ok_or_else(|| Error::Invalid("思考摘要索引溢出".into()))?,
    ))
}
/// Responses 输出项及内容块索引映射到单候选位置。
fn key(item: u64, part: u64) -> Key {
    Key {
        candidate: 0,
        item,
        part,
    }
}
/// 思考摘要和正文只提取文本，不暴露签名或其他原生字段。
fn reasoning_text(part: &ReasoningPart) -> &str {
    match part {
        ReasoningPart::ReasoningText { text, .. } | ReasoningPart::SummaryText { text, .. } => text,
    }
}

/// text.done 与 content_part.done 可以连续到达，仅关闭一次 IR 内容块。
fn end_part(key: Key, context: &mut Context<'_>) -> Result<()> {
    if context.state.part(key).is_some_and(|part| !part.ended) {
        context.emit(Event::PartEnd(key))?;
    }
    Ok(())
}
