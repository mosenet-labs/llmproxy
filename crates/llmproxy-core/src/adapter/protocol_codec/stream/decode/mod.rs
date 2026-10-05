//! 四协议共享生命周期校验，协议模块只负责各自的事件语义。

mod chat;
mod gemini;
mod messages;
mod responses;

use crate::{
    adapter::{Error, Result},
    ir::{
        response::{FinishReason, Status},
        stream::{Event, Head, Key, Limits, Metadata, State},
    },
    protocol::{Protocol, stream::Event as Raw},
};

/// 单次响应的协议事件解码器；错误后不能继续复用，避免部分状态被当成成功。
pub struct Decoder {
    protocol: Protocol,
    source: Source,
    state: State,
    failed: bool,
    entries: usize,
}

enum Source {
    Chat,
    Responses(responses::Decoder),
    Messages(messages::Decoder),
    Gemini(gemini::Decoder),
}

impl Decoder {
    /// 创建来源协议对应的解码状态，容量由 HTTP 接入层传入。
    pub(crate) fn new(protocol: Protocol, limits: Limits) -> Self {
        let source = match protocol {
            Protocol::OpenAiChat => Source::Chat,
            Protocol::OpenAiResponses => Source::Responses(Default::default()),
            Protocol::AnthropicMessages => Source::Messages(Default::default()),
            Protocol::Gemini => Source::Gemini(Default::default()),
        };
        Self {
            protocol,
            source,
            state: State::new(limits),
            failed: false,
            entries: limits.entries,
        }
    }

    /// 读取一条类型化事件，立即返回可交付的语义增量；不等待完整响应。
    pub fn push(&mut self, event: &Raw) -> Result<Vec<Event>> {
        if self.failed || event.protocol() != self.protocol {
            self.failed = true;
            return Err(Error::Invalid("流式解码器不可复用或协议不匹配".into()));
        }
        let result = self.decode(event);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// 读取已校验的统计与结束状态，不包含累计文本。
    pub fn state(&self) -> &State {
        &self.state
    }

    /// 将来源分派和 IR 校验置于同一个错误边界。
    fn decode(&mut self, event: &Raw) -> Result<Vec<Event>> {
        if self.state.ended().is_some() {
            return if matches!(event, Raw::End(_)) {
                Ok(Vec::new())
            } else {
                Err(Error::Invalid("流式响应结束后仍有正文".into()))
            };
        }
        let mut context = Context {
            state: &mut self.state,
            events: Vec::new(),
            entries: self.entries,
        };
        match (&mut self.source, event) {
            (Source::Chat, Raw::Chat(chunk)) => chat::decode(chunk, &mut context)?,
            (Source::Responses(source), Raw::Responses(event)) => {
                source.decode(event, &mut context)?
            }
            (Source::Messages(source), Raw::Messages(event)) => {
                source.decode(event, &mut context)?
            }
            (Source::Gemini(source), Raw::Gemini(chunk)) => source.decode(chunk, &mut context)?,
            (Source::Chat | Source::Gemini(_), Raw::End(_)) => {
                context.emit(Event::End(Status::Completed))?
            }
            (_, Raw::End(_)) => return Err(Error::Invalid("上游在协议结束事件之前断开".into())),
            _ => return Err(Error::Invalid("流式事件与来源状态不匹配".into())),
        }
        Ok(context.events)
    }
}

/// 发出事件时同步校验；协议实现不能绕过顺序和局部缓冲上限。
struct Context<'a> {
    state: &'a mut State,
    events: Vec<Event>,
    entries: usize,
}

impl Context<'_> {
    /// 状态校验成功后再把事件交给调用方。
    fn emit(&mut self, event: Event) -> Result<()> {
        self.state
            .accept(&event)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        self.events.push(event);
        Ok(())
    }
    /// 首个语义载体开始响应，后续载体不重复创建响应。
    fn start(&mut self, metadata: Metadata) -> Result<()> {
        if self.state.metadata().is_none() {
            self.emit(Event::Start(metadata))?;
        }
        Ok(())
    }
    /// 首次遇到候选时创建；已结束的候选不能追加内容。
    fn candidate(&mut self, index: u64) -> Result<()> {
        match self.state.candidate(index) {
            None => self.emit(Event::CandidateStart(index)),
            Some(None) => Ok(()),
            Some(Some(_)) => Err(Error::Invalid("已结束候选仍有增量".into())),
        }
    }
    /// Text/Refusal/Reasoning 类型必须保持一致；工具头由 ToolDelta 继续补齐。
    fn part(&mut self, key: Key, head: Head) -> Result<()> {
        if let Some(current) = self.state.part(key) {
            if current.ended
                || std::mem::discriminant(&current.head) != std::mem::discriminant(&head)
            {
                return Err(Error::Invalid("流式内容块类型改变或重复使用索引".into()));
            }
            return Ok(());
        }
        self.emit(Event::PartStart { key, head })
    }
    /// 字符串增量不进入累计文本缓冲。
    fn text(&mut self, key: Key, head: Head, text: &str) -> Result<()> {
        self.part(key, head)?;
        if !text.is_empty() {
            self.emit(Event::TextDelta {
                key,
                text: text.into(),
            })?;
        }
        Ok(())
    }
    /// 某些协议仅有候选结束事件，需要显式关闭该候选的活动内容块。
    fn close_candidate(&mut self, index: u64, reason: FinishReason) -> Result<()> {
        let keys = self.state.active_keys(index).collect::<Vec<_>>();
        for key in keys {
            self.emit(Event::PartEnd(key))?;
        }
        self.emit(Event::CandidateEnd { index, reason })
    }
}
