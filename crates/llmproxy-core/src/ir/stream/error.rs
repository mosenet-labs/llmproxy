//! 生命周期错误只说明边界，不包含 Provider 的正文或参数。

/// 事件状态校验失败，独立于协议适配和 HTTP 错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// 事件不符合当前生命周期。
    Order,
    /// 内容块不存在、已结束，或所属候选已结束。
    InactivePart,
    /// 工具及签名的保留字节超过上限。
    BufferLimit,
    /// 候选及内容块索引超过上限。
    EntryLimit,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Order => "流式事件顺序无效",
            Self::InactivePart => "流式内容块不存在或已经结束",
            Self::BufferLimit => "流式工具缓冲超过上限",
            Self::EntryLimit => "流式内容索引超过上限",
        })
    }
}

impl std::error::Error for Error {}
