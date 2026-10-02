//! 整体非流式响应的中间表示；用量可供后续逐轮展示。

use serde::{Deserialize, Serialize};

use super::{Message, source::Source};
use crate::{ir::usage::Usage, protocol::Protocol};

/// 一次完整响应的消息和用量投影，以及可同协议回写的来源类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// 按候选或输出项顺序排列的消息。
    pub messages: Vec<Message>,
    /// 上游报告的本次用量；缺失与明确报告零区分。
    pub usage: Option<Usage>,
    /// 未规范化的候选、工具输出及供应商扩展字段。
    pub(crate) source: Source,
}

impl Response {
    /// 返回原报文使用的协议。
    pub fn source_protocol(&self) -> Protocol {
        self.source.protocol()
    }

    /// 返回本次响应的缓存读写用量；未报告 usage 时返回 `None`。
    pub fn cache_usage(&self) -> Option<&crate::ir::cache::CacheUsage> {
        self.usage.as_ref().map(|usage| &usage.cache)
    }
}
