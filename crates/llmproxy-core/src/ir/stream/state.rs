//! 事件顺序校验与有界工具状态；普通文本不在此累积整次响应。
use std::collections::BTreeMap;

use super::{Error, Event, Head, Key, Metadata, UsageState};
use crate::ir::response::{FinishReason, Status};

type Result<T> = std::result::Result<T, Error>;

/// 单次流式转换保留状态的上限。
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// 工具标识、名称、参数和签名的字节预算；目标编码也用此上限约束终态正文快照。
    pub buffered_bytes: usize,
    /// 候选及内容块的总数量上限，包括已结束的索引记录。
    pub entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            buffered_bytes: 8 * 1024 * 1024,
            entries: 1024,
        }
    }
}

/// 当前块的最小状态，文本正文不在这里保留。
#[derive(Clone, Debug)]
pub struct PartState {
    /// 内容类型及工具头的累计字段。
    pub head: Head,
    /// 尚未完成的工具参数，不强行解析局部 JSON。
    pub arguments: String,
    /// 内容块是否已经结束。
    pub ended: bool,
    held_bytes: usize,
}

/// 仅描述事件生命周期，不涉及 SSE、HTTP、数据库和 Provider 路由。
#[derive(Debug)]
pub struct State {
    metadata: Option<Metadata>,
    candidates: BTreeMap<u64, Option<FinishReason>>,
    parts: BTreeMap<Key, PartState>,
    usage: UsageState,
    failed: bool,
    ended: Option<Status>,
    buffered_bytes: usize,
    limits: Limits,
}

impl State {
    /// 创建单次响应状态，不能复用于另一个响应。
    pub fn new(limits: Limits) -> Self {
        Self {
            metadata: None,
            candidates: BTreeMap::new(),
            parts: BTreeMap::new(),
            usage: UsageState::default(),
            failed: false,
            ended: None,
            buffered_bytes: 0,
            limits,
        }
    }

    /// 获取本次响应的通用外壳。
    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }
    /// 获取一个内容块；参数只能在 PartEnd 之前读取。
    pub fn part(&self, key: Key) -> Option<&PartState> {
        self.parts.get(&key)
    }
    /// 遍历某候选尚未结束的内容块，用于没有逐块结束事件的来源协议。
    pub fn active_keys(&self, candidate: u64) -> impl Iterator<Item = Key> + '_ {
        self.parts.iter().filter_map(move |(key, part)| {
            (key.candidate == candidate && !part.ended).then_some(*key)
        })
    }
    /// 获取按累计语义合并的实际统计。
    pub fn usage(&self) -> &UsageState {
        &self.usage
    }
    /// 查询候选是否已结束以及实际停止原因。
    pub fn candidate(&self, index: u64) -> Option<Option<FinishReason>> {
        self.candidates.get(&index).copied()
    }
    /// 获取整体终止状态。
    pub fn ended(&self) -> Option<Status> {
        self.ended
    }
    /// 获取当前保留的工具及签名字节数。
    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    /// 接受一条事件；顺序和容量验证先于状态写入，拒绝后可检查原状态。
    pub fn accept(&mut self, event: &Event) -> Result<()> {
        if let Some(status) = self.ended {
            return if matches!(event, Event::End(next) if *next == status) {
                Ok(())
            } else {
                invalid()
            };
        }
        if let Event::Start(metadata) = event {
            if self.metadata.is_some() {
                return invalid();
            }
            self.metadata = Some(metadata.clone());
            return Ok(());
        }
        if self.metadata.is_none()
            && !matches!(
                event,
                Event::Heartbeat | Event::Unknown { .. } | Event::Diagnostic { .. }
            )
        {
            return invalid();
        }
        if self.failed
            && !matches!(
                event,
                Event::Usage(_) | Event::Failure(_) | Event::End(_) | Event::Diagnostic { .. }
            )
        {
            return invalid();
        }
        match event {
            Event::Start(_) => unreachable!(),
            Event::CandidateStart(index) => {
                if self.candidates.contains_key(index) {
                    return invalid();
                }
                self.check_entries()?;
                self.candidates.insert(*index, None);
            }
            Event::PartStart { key, head } => {
                if !matches!(self.candidates.get(&key.candidate), Some(None))
                    || self.parts.contains_key(key)
                {
                    return invalid();
                }
                self.check_entries()?;
                let held_bytes = match head {
                    Head::Tool(tool) => {
                        tool.id.as_ref().map_or(0, String::len)
                            + tool.name.as_ref().map_or(0, String::len)
                    }
                    _ => 0,
                };
                self.reserve(held_bytes)?;
                self.parts.insert(
                    *key,
                    PartState {
                        head: head.clone(),
                        arguments: String::new(),
                        ended: false,
                        held_bytes,
                    },
                );
                self.buffered_bytes += held_bytes;
            }
            Event::TextDelta { key, .. } => {
                if !matches!(
                    self.active(*key)?.head,
                    Head::Text | Head::Refusal | Head::Reasoning
                ) {
                    return invalid();
                }
            }
            Event::ToolDelta {
                key,
                id,
                name,
                arguments,
            } => {
                let part = self.active(*key)?;
                let Head::Tool(tool) = &part.head else {
                    return invalid();
                };
                if id
                    .as_ref()
                    .zip(tool.id.as_ref())
                    .is_some_and(|(next, old)| next != old)
                {
                    return invalid();
                }
                let added = if tool.id.is_none() {
                    id.as_ref().map_or(0, String::len)
                } else {
                    0
                } + name.as_ref().map_or(0, String::len)
                    + arguments.as_ref().map_or(0, String::len);
                self.reserve(added)?;
                let part = self.parts.get_mut(key).expect("active 已确认内容块");
                let Head::Tool(tool) = &mut part.head else {
                    unreachable!()
                };
                if tool.id.is_none() {
                    tool.id.clone_from(id);
                }
                if let Some(name) = name {
                    tool.name.get_or_insert_with(String::new).push_str(name);
                }
                if let Some(arguments) = arguments {
                    part.arguments.push_str(arguments);
                }
                part.held_bytes += added;
                self.buffered_bytes += added;
            }
            Event::Signature { key, data, .. } => {
                if !matches!(
                    self.active(*key)?.head,
                    Head::Text | Head::Reasoning | Head::Tool(_)
                ) {
                    return invalid();
                }
                self.reserve(data.len())?;
                self.parts
                    .get_mut(key)
                    .expect("active 已确认内容块")
                    .held_bytes += data.len();
                self.buffered_bytes += data.len();
            }
            Event::PartEnd(key) => {
                let part = self.active(*key)?;
                if matches!(&part.head, Head::Tool(tool) if tool.name.as_ref().is_none_or(|name| name.is_empty()))
                {
                    return invalid();
                }
                let part = self.parts.get_mut(key).expect("active 已确认内容块");
                self.buffered_bytes -= part.held_bytes;
                part.held_bytes = 0;
                part.arguments = String::new();
                if let Head::Tool(tool) = &mut part.head {
                    *tool = Default::default();
                }
                part.ended = true;
            }
            Event::Media { key, .. } | Event::ServerOutput { key, .. } => {
                if !matches!(self.candidates.get(&key.candidate), Some(None))
                    || self.parts.contains_key(key)
                {
                    return invalid();
                }
                self.check_entries()?;
                self.parts.insert(
                    *key,
                    PartState {
                        head: Head::Text,
                        arguments: String::new(),
                        ended: true,
                        held_bytes: 0,
                    },
                );
            }
            Event::Annotation { key, .. } => {
                // 完成快照可以补齐已结束文本块的引用，候选结束后不能再追加。
                if !self.parts.contains_key(key)
                    || !matches!(self.candidates.get(&key.candidate), Some(None))
                {
                    return invalid();
                }
            }
            Event::CandidateEnd { index, reason } => {
                let Some(current) = self.candidates.get(index) else {
                    return invalid();
                };
                if current.is_some()
                    || self
                        .parts
                        .iter()
                        .any(|(key, part)| key.candidate == *index && !part.ended)
                {
                    return invalid();
                }
                self.candidates.insert(*index, Some(*reason));
            }
            Event::Usage(usage) => self.usage.update(usage),
            Event::Failure(_) => self.failed = true,
            Event::End(status) => {
                if matches!(status, Status::InProgress | Status::Unknown) {
                    return invalid();
                }
                if matches!(status, Status::Completed | Status::Incomplete)
                    && (self.failed
                        || self.candidates.is_empty()
                        || self.candidates.values().any(Option::is_none))
                {
                    return invalid();
                }
                self.ended = Some(*status);
                self.buffered_bytes = 0;
                for part in self.parts.values_mut() {
                    part.arguments = String::new();
                    part.held_bytes = 0;
                    if let Head::Tool(tool) = &mut part.head {
                        *tool = Default::default();
                    }
                }
            }
            Event::Heartbeat
            | Event::Native(_)
            | Event::Unknown { .. }
            | Event::Diagnostic { .. } => {}
        }
        Ok(())
    }

    /// 只返回尚未结束且候选仍在生成的内容块。
    fn active(&self, key: Key) -> Result<&PartState> {
        self.parts
            .get(&key)
            .filter(|part| !part.ended && matches!(self.candidates.get(&key.candidate), Some(None)))
            .ok_or(Error::InactivePart)
    }
    /// 同时约束活动状态和结束索引，避免无限创建零长度块。
    fn check_entries(&self) -> Result<()> {
        if self.parts.len() + self.candidates.len() >= self.limits.entries {
            Err(Error::EntryLimit)
        } else {
            Ok(())
        }
    }
    /// 溢出和超过局部缓冲预算都明确拒绝。
    fn reserve(&self, bytes: usize) -> Result<()> {
        if self
            .buffered_bytes
            .checked_add(bytes)
            .is_none_or(|n| n > self.limits.buffered_bytes)
        {
            Err(Error::BufferLimit)
        } else {
            Ok(())
        }
    }
}

/// 不把 Provider 的字段值带入顺序错误。
fn invalid<T>() -> Result<T> {
    Err(Error::Order)
}
