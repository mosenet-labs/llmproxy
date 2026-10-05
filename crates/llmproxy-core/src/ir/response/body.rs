//! 整体非流式响应的中间表示；用量可供后续逐轮展示。

use serde::{Deserialize, Serialize};

use super::{Candidate, Failure, Item, Message, Status, source::Source};
use crate::{ir::usage::Usage, protocol::Protocol};

/// 一次完整响应的消息和用量投影，以及可同协议回写的来源类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// 响应模型标识。
    pub model: Option<String>,
    /// 来源响应 ID；跨协议时可由 Gateway 明确覆盖。
    pub id: Option<String>,
    /// Unix 秒级创建时间；来源未提供时保持缺失。
    pub created_at: Option<i64>,
    /// 整体生成状态。
    pub status: Status,
    /// 生成失败详情；状态与错误分别保存，避免 HTTP 200 被误认为生成成功。
    #[serde(default)]
    pub failure: Option<Failure>,
    /// 候选边界及各自的结束原因。
    pub candidates: Vec<Candidate>,
    /// 尚未进入通用语义的字段及跨协议处理规则。
    pub diagnostics: Vec<crate::ir::diagnostic::Diagnostic>,
    /// 按候选或输出项顺序排列的消息。
    pub messages: Vec<Message>,
    /// 消息及独立输出项的原始顺序。
    pub items: Vec<Item>,
    /// 上游报告的本次用量；缺失与明确报告零区分。
    pub usage: Option<Usage>,
    /// 未规范化的候选、工具输出及供应商扩展字段。
    pub(crate) source: Option<Source>,
    /// 来源标识仅用于诊断，不要求存在来源报文。
    pub(crate) origin: Protocol,
}

impl Response {
    /// 创建无需来源报文即可编码的整体响应 IR。
    pub fn new(origin: Protocol) -> Self {
        Self {
            model: None,
            id: None,
            created_at: None,
            status: Status::Unknown,
            failure: None,
            candidates: Vec::new(),
            diagnostics: Vec::new(),
            messages: Vec::new(),
            items: Vec::new(),
            usage: None,
            source: None,
            origin,
        }
    }

    /// 释放同协议往返副本；此后仅可选择 Rebuild，以通用字段构造目标协议。
    pub fn without_source(mut self) -> Self {
        self.source = None;
        self
    }

    /// 返回原报文使用的协议。
    pub fn source_protocol(&self) -> Protocol {
        self.origin
    }

    /// 返回本次响应的缓存读写用量；未报告 usage 时返回 `None`。
    pub fn cache_usage(&self) -> Option<&crate::ir::cache::CacheUsage> {
        self.usage.as_ref().map(|usage| &usage.cache)
    }
}
