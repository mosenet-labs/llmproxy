//! Provider management persistence. Public views never contain stored credentials.

mod crypto;
mod model;
mod store;

use std::fmt;

use llmproxy_core::protocol::Protocol;

pub use store::ProviderStore;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Validation(String),
    Conflict(String),
    Configuration(&'static str),
    NotFound,
    Internal,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) | Self::Conflict(message) => formatter.write_str(message),
            Self::Configuration(message) => formatter.write_str(message),
            Self::NotFound => formatter.write_str("Provider 不存在或已被删除"),
            Self::Internal => formatter.write_str("Provider 存储操作失败，请检查服务配置"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<toasty::Error> for StoreError {
    fn from(error: toasty::Error) -> Self {
        // Driver errors can contain SQL values or connection credentials.
        if error.is_condition_failed() || error.is_serialization_failure() {
            Self::Conflict("Provider 已被其他操作修改，请刷新后重试".into())
        } else if error.is_record_not_found() {
            Self::NotFound
        } else {
            Self::Internal
        }
    }
}

/// Write-only credential input. An empty key preserves the key when editing.
#[derive(Clone)]
pub struct ProviderInput {
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub api_key: String,
    pub enabled: bool,
    pub anthropic_version: Option<String>,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub struct ProviderView {
    pub id: i64,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub enabled: bool,
    pub anthropic_version: Option<String>,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    pub active: bool,
    pub version: u64,
    pub key_configured: bool,
    pub updated_at: i64,
}

/// Gateway-only configuration. This type intentionally has no Debug or Serialize.
#[derive(Clone)]
pub struct ActiveProvider {
    pub id: i64,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub secret: String,
    pub anthropic_version: Option<String>,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}
