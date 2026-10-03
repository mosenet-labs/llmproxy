//! 整体响应状态和候选边界。

use serde::{Deserialize, Serialize};

/// 一次生成的执行状态，独立于 HTTP 状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// 完成生成；候选自身仍可能因长度或过滤而结束。
    Completed,
    /// 内容尚未完整生成。
    Incomplete,
    /// 生成失败。
    Failed,
    /// 排队或生成中。
    InProgress,
    /// 来源未提供可识别状态。
    #[default]
    Unknown,
}

/// 候选的终止原因；当前跨协议仅编码正常结束和工具调用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// 自然结束或命中停止序列。
    Stop,
    /// 等待客户端执行工具。
    ToolCall,
    /// 达到生成上限。
    Length,
    /// 被安全或内容规则过滤。
    Filtered,
    /// 未知或未提供。
    #[default]
    Unknown,
}

/// 同一候选包含的有序输出项，避免把多个候选合并成一条回复。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// 来源候选序号。
    pub index: u64,
    /// 指向 Response.items 的索引，按候选内部输出顺序排列。
    pub items: Vec<usize>,
    /// 此候选的结束原因。
    pub finish_reason: FinishReason,
}
