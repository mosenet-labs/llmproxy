//! 生成失败的通用信息；HTTP 错误外壳由接入边界构造。
use serde::{Deserialize, Serialize};

/// Provider 报告的生成失败，独立于传输是否返回 HTTP 200。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// 来源错误代码；未知代码保持原值，不猜测可重试性。
    pub code: String,
    /// 来源错误说明；可能含私有信息，不能直接写日志或跨 Provider 返回客户端。
    pub message: String,
}
