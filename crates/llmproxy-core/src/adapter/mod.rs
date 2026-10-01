//! 原始协议和中间表示之间的转换。

pub mod request;
pub mod response;
mod wire;

/// 消息适配过程中的数据错误或目标协议能力限制。
#[derive(Debug)]
pub enum Error {
    /// 原始消息或 IR 与期望的形状不一致。
    Invalid(String),
    /// 目标协议无法安全表达此语义。
    Unsupported(String),
    /// 原始协议 DTO 的 JSON 转换失败。
    Json(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Unsupported(message) => f.write_str(message),
            Self::Json(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

type Result<T> = std::result::Result<T, Error>;
