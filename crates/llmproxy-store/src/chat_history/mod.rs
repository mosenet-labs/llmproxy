//! 会话、轮次快照和来源统计的应用接口。
mod summary;
use llmproxy_core::{
    conversation::{Reply, Selection},
    ir::{request::controls::Reasoning, usage::Usage},
    protocol::Protocol,
    thinking::Choice,
};
use serde::{Deserialize, Serialize};
pub use summary::{Counter, ModelTotals, Summary, Totals};

/// 本地控制台首版使用稳定工作区归属。
pub const OWNER: &str = "workspace";
/// 服务端业务请求关联头，在发给 Provider 前移除。
pub const REQUEST_HEADER: &str = "x-llmproxy-history-request";

/// 轮次执行状态；用量完整程度单独记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Generating,
    Completed,
    Incomplete,
    Failed,
    Cancelled,
    Interrupted,
}
impl Status {
    /// 数据库和页面共用稳定状态名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generating => "generating",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
    /// 只有有效结束的回复可进入下一轮上下文。
    pub fn usable(self) -> bool {
        matches!(self, Self::Completed | Self::Incomplete)
    }
}
/// 未报告、部分快照、协议最终计数分别保存。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageState {
    Unreported,
    Partial,
    Final,
}
impl UsageState {
    /// 数据库存储使用与序列化一致的稳定名称。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreported => "unreported",
            Self::Partial => "partial",
            Self::Final => "final",
        }
    }
    /// 中文统计标签，不将缺失用量显示为零。
    pub fn label(self) -> &'static str {
        match self {
            Self::Unreported => "用量未报告",
            Self::Partial => "部分用量",
            Self::Final => "最终用量",
        }
    }
}
/// 会话选择及偏好，更新不会修改旧轮快照。
#[derive(Clone, Debug)]
pub struct Conversation {
    /// 跨重启稳定的随机记录标识。
    pub id: String,
    /// 第一轮输入的简短标题。
    pub title: String,
    /// 本轮固定的模型／路由与客户端协议选择。
    pub selection: Selection,
    /// 用户提交时的思考偏好，不推测 Provider 默认行为。
    pub thinking: Choice,
    /// 正在生成的轮次标识；数据库据此阻止并发生成。
    pub active_turn: Option<String>,
    /// 归档后仍可查阅，恢复后才允许继续对话。
    pub archived: bool,
    /// 会话创建时间，Unix 秒。
    pub created_at: i64,
    /// 最后一次选择或轮次变更时间，Unix 秒。
    pub updated_at: i64,
}
/// 实际路由快照，模型配置删除后仍能展示。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActualCall {
    /// 实际 Provider 标识，不设配置外键。
    pub provider_id: i64,
    /// 实际 Provider 名称快照。
    pub provider_name: String,
    /// 实际发给 Provider 的模型标识。
    pub upstream_model: String,
    /// 实际 Provider 的协议。
    pub protocol: Protocol,
    /// 解码后生效的思考配置；透传默认配置时不推测。
    pub reasoning: Option<Reasoning>,
    /// 来源响应实际报告的模型。
    pub reported_model: Option<String>,
}
/// 数据库中的一轮输入、回复与来源统计。
#[derive(Clone, Debug)]
pub struct Turn {
    /// Gateway 已认领实际请求。
    pub call_started: bool,
    /// 来源统计已经落库，独立于正文终态。
    pub call_finished: bool,
    /// 跨重启稳定的随机记录标识。
    pub id: String,
    /// 所属会话标识。
    pub conversation_id: String,
    /// 会话内单调递增的轮次顺序。
    pub sequence: i64,
    /// 本轮固定的模型／路由与客户端协议选择。
    pub selection: Selection,
    /// 请求提交时的模型别名或路由名快照。
    pub alias: String,
    /// 用户提交时的思考偏好，不推测 Provider 默认行为。
    pub thinking: Choice,
    /// 用户输入明文，仅在 Store 边界解密。
    pub prompt: String,
    /// 可见回复和可用于续接的类型化 IR；生成中可缺失。
    pub reply: Option<Reply>,
    /// 执行终态，与用量是否完整分开。
    pub status: Status,
    /// 安全错误说明，用于展示而不进入提示词。
    pub error: Option<String>,
    /// Gateway 选定的实际 Provider 与模型快照。
    pub actual: Option<ActualCall>,
    /// 来源 IR 用量，完整保存缓存 TTL 和模态明细。
    pub usage: Option<Usage>,
    /// 未报告、部分或最终统计的完整程度。
    pub usage_state: UsageState,
    /// 提交时间，Unix 秒。
    pub started_at: i64,
    /// 结束时间，生成中为空。
    pub ended_at: Option<i64>,
}
/// 开始轮次前固定的输入快照；ID 同时作为本轮业务请求 ID。
pub struct Input {
    /// 跨重启稳定的随机记录标识。
    pub id: String,
    /// 所属会话标识。
    pub conversation_id: String,
    /// 本轮固定的模型／路由与客户端协议选择。
    pub selection: Selection,
    /// 请求提交时的模型别名或路由名快照。
    pub alias: String,
    /// 用户提交时的思考偏好，不推测 Provider 默认行为。
    pub thinking: Choice,
    /// 用户输入明文，仅在 Store 边界解密。
    pub prompt: String,
}
/// Console 只更新内容和状态，不能覆盖 Gateway 保存的 Usage。
#[derive(Clone)]
pub struct Completion {
    /// 执行终态，与用量是否完整分开。
    pub status: Status,
    /// 可见回复和可用于续接的类型化 IR；生成中可缺失。
    pub reply: Reply,
    /// 安全错误说明，用于展示而不进入提示词。
    pub error: Option<String>,
}
