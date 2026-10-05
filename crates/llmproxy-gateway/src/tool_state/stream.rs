//! 仅暂存 Gemini 的函数调用组；文本仍立即交付，引用在持久化成功后才发布。
use super::{Context, Pending, Result, failure};
use llmproxy_core::{
    ir::stream::{Event, Head, Key},
    protocol::{
        OptionalNullable as O, Protocol, gemini::request::message::Part, stream::Event as Raw,
    },
};
use std::collections::BTreeMap;

pub struct Capture {
    context: Context,
    target: Protocol,
    parts: BTreeMap<Key, Part>,
    events: BTreeMap<u64, Vec<Event>>,
    bytes: usize,
}
impl Capture {
    /// 当前请求使用已有鉴权作用域和加密数据库，不引入额外会话标识。
    pub fn new(context: Context, target: Protocol) -> Self {
        Self {
            context,
            target,
            parts: BTreeMap::new(),
            events: BTreeMap::new(),
            bytes: 0,
        }
    }
    /// 对齐本帧类型化函数 Part 与 codec 分配的 Key，晚到签名在 IR 中更新原 Part。
    pub fn observe(&mut self, raw: &Raw, events: &[Event]) -> Result<()> {
        let Raw::Gemini(raw) = raw else {
            return Ok(());
        };
        let mut originals = raw
            .candidates
            .as_option()
            .into_iter()
            .flatten()
            .enumerate()
            .flat_map(|(position, candidate)| {
                let index = candidate
                    .index
                    .as_option()
                    .copied()
                    .unwrap_or(position as u64);
                candidate
                    .content
                    .as_option()
                    .into_iter()
                    .flat_map(move |message| {
                        message
                            .parts
                            .iter()
                            .filter(|part| part.function_call.as_option().is_some())
                            .map(move |part| (index, part))
                    })
            });
        for event in events {
            if let Event::PartStart {
                key,
                head: Head::Tool(_),
            } = event
            {
                let (index, part) = originals
                    .next()
                    .ok_or_else(|| failure("invalid_stream_group"))?;
                if index != key.candidate {
                    return Err(failure("invalid_stream_group"));
                }
                self.bytes = self.bytes.saturating_add(serde_json::to_vec(part)?.len());
                self.parts.insert(*key, part.clone());
            }
            if let Event::Signature {
                key,
                protocol: Protocol::Gemini,
                data,
            } = event
                && let Some(part) = self.parts.get_mut(key)
            {
                self.bytes = self.bytes.saturating_sub(serde_json::to_vec(part)?.len());
                part.thought_signature = O::Value(data.clone());
                self.bytes = self.bytes.saturating_add(serde_json::to_vec(part)?.len());
            }
        }
        self.check_limit()
    }
    /// 函数事件等待候选调用组完整；候选结束时返回待编码事件和待提交条目。
    pub fn take(&mut self, event: Event) -> Result<(Vec<Event>, Option<Pending>)> {
        let key = match &event {
            Event::PartStart { key, .. }
            | Event::ToolDelta { key, .. }
            | Event::Signature { key, .. } => Some(*key),
            Event::PartEnd(key) => Some(*key),
            _ => None,
        };
        if let Some(key) = key
            && self.parts.contains_key(&key)
        {
            self.bytes = self.bytes.saturating_add(serde_json::to_vec(&event)?.len());
            self.events.entry(key.candidate).or_default().push(event);
            self.check_limit()?;
            return Ok((Vec::new(), None));
        }
        if let Event::CandidateEnd { index, .. } = event {
            let mut events = self.events.remove(&index).unwrap_or_default();
            let keys: Vec<_> = self
                .parts
                .keys()
                .filter(|key| key.candidate == index)
                .copied()
                .collect();
            let parts: Vec<_> = keys
                .iter()
                .map(|key| self.parts.remove(key).unwrap())
                .collect();
            for part in &parts {
                self.bytes = self.bytes.saturating_sub(serde_json::to_vec(part)?.len());
            }
            for event in &events {
                self.bytes = self.bytes.saturating_sub(serde_json::to_vec(event)?.len());
            }
            let selected = self.target == Protocol::OpenAiChat || index == 0;
            let pending = if selected
                && parts.iter().any(|part| {
                    part.thought_signature
                        .as_option()
                        .is_some_and(|s| !s.is_empty())
                }) {
                let pending = self
                    .context
                    .capture_group(&parts.iter().collect::<Vec<_>>())?;
                for (key, (id, _)) in keys.iter().zip(&pending.entries) {
                    for event in &mut events {
                        match event {
                            Event::PartStart {
                                key: k,
                                head: Head::Tool(head),
                            } if k == key => head.id = Some(id.clone()),
                            Event::ToolDelta {
                                key: k,
                                id: Some(old),
                                ..
                            } if k == key => *old = id.clone(),
                            _ => {}
                        }
                    }
                }
                Some(pending)
            } else {
                None
            };
            events.push(event);
            self.check_limit()?;
            return Ok((events, pending));
        }
        Ok((vec![event], None))
    }
    /// 本地编码成功后提交组，并等待数据库完成；持有输出期间尚未发送任何引用。
    pub async fn persist(&self, pending: Pending) -> pingora::Result<()> {
        pending.commit().map_err(|_| {
            pingora::Error::explain(
                pingora::ErrorType::HTTPStatus(502),
                "tool state capacity exceeded",
            )
        })?;
        self.context.persist_response().await
    }
    /// 原生 Part 和待编码 IR 增量共用预算，防止签名等待过程无界增长。
    fn check_limit(&self) -> Result<()> {
        if self.bytes > super::super::transform::MAX_BUFFERED_BODY
            || self.parts.len() + self.events.values().map(Vec::len).sum::<usize>() > 4096
        {
            return Err(failure("stream_capacity"));
        }
        Ok(())
    }
}
