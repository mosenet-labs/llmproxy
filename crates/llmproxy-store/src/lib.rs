//! Provider management persistence. Public views never contain stored credentials.

mod crypto;
mod database;
mod model;
mod store;

use std::fmt;

use llmproxy_core::protocol::Protocol;

pub use database::{Backend, DatabaseConfig};
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
    pub paths: ProviderPaths,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub api_key: String,
    pub enabled: bool,
    pub models_path: String,
    pub models_protocol: Protocol,
    pub anthropic_version: Option<String>,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

#[derive(Clone, Debug)]
pub struct ProviderView {
    pub id: i64,
    pub name: String,
    pub paths: ProviderPaths,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub enabled: bool,
    pub models_path: String,
    pub models_protocol: Protocol,
    pub models_probe_status: ProbeStatus,
    pub anthropic_version: Option<String>,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    pub active: bool,
    pub active_protocols: Vec<Protocol>,
    pub version: u64,
    pub key_configured: bool,
    pub updated_at: i64,
}

/// Gateway-only configuration. This type intentionally has no Debug or Serialize.
#[derive(Clone)]
pub struct ActiveProvider {
    pub id: i64,
    pub protocol: Protocol,
    pub upstream_path: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub secret: String,
    pub anthropic_version: Option<String>,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderPaths {
    pub openai_chat: Option<String>,
    pub openai_responses: Option<String>,
    pub anthropic_messages: Option<String>,
}

impl ProviderPaths {
    pub fn single(protocol: Protocol) -> Self {
        let mut paths = Self::default();
        match protocol {
            Protocol::OpenAiChat => paths.openai_chat = Some(protocol.upstream_path().into()),
            Protocol::OpenAiResponses => {
                paths.openai_responses = Some(protocol.upstream_path().into())
            }
            Protocol::AnthropicMessages => {
                paths.anthropic_messages = Some(protocol.upstream_path().into())
            }
        }
        paths
    }

    pub fn get(&self, protocol: Protocol) -> Option<&str> {
        match protocol {
            Protocol::OpenAiChat => self.openai_chat.as_deref(),
            Protocol::OpenAiResponses => self.openai_responses.as_deref(),
            Protocol::AnthropicMessages => self.anthropic_messages.as_deref(),
        }
    }

    pub fn supported(&self) -> Vec<Protocol> {
        [
            Protocol::OpenAiChat,
            Protocol::OpenAiResponses,
            Protocol::AnthropicMessages,
        ]
        .into_iter()
        .filter(|protocol| self.get(*protocol).is_some())
        .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProbeStatus {
    #[default]
    Unprobed,
    Success,
    Failure,
}

impl ProbeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unprobed => "unprobed",
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "unprobed" => Ok(Self::Unprobed),
            "success" => Ok(Self::Success),
            "failure" => Ok(Self::Failure),
            _ => Err(StoreError::Validation("无效的模型探测状态".into())),
        }
    }
}

/// Server-only target for model discovery. This type has no Debug or Serialize.
pub struct ModelProbeTarget {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub path: String,
    pub protocol: Protocol,
    pub secret: String,
    pub anthropic_version: Option<String>,
}
