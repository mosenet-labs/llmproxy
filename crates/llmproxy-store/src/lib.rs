//! Provider management persistence. Public views never contain stored credentials.

pub mod chat_history;
mod crypto;
mod database;
pub mod health;
mod holiday;
mod model;
mod pricing;
mod store;

use std::fmt;

use llmproxy_core::protocol::Protocol;

pub use llmproxy_core::protocol::MessagesAuth;

pub use database::{Backend, DatabaseConfig};
pub use holiday::{HolidayDate, HolidayKind};
pub use pricing::{
    PriceConditions, PriceItem, PricePlanInput, PricePlanView, PriceRule, PriceSchedule,
    PriceSource, TimeBand, WeeklyPeakWindow, resolve_time_band,
};
pub use store::ProviderStore;

#[derive(Clone, Debug)]
pub struct SubscriptionNodeView {
    pub node_id: String,
    pub name: String,
    pub provider_name: Option<String>,
    pub backend: String,
    pub models: Vec<String>,
    pub concurrency: u64,
    pub enabled: bool,
    pub provider_id: Option<i64>,
    pub version: u64,
}

/// 服务端普通 Provider 转接目标；密钥不进入公开视图。
#[derive(Clone)]
pub struct SubscriptionRelayTarget {
    pub host: String,
    pub port: u16,
    pub key: String,
}

/// Gateway 内部使用的工具续接记录；载荷含签名，不实现 Debug 或 Serialize。
pub struct ToolContinuation {
    pub id: String,
    pub scope: String,
    pub payload: String,
}

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
    pub messages_auth: MessagesAuth,
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
    pub messages_auth: MessagesAuth,
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
    pub name: String,
    pub id: i64,
    pub protocol: Protocol,
    pub upstream_path: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub secret: String,
    pub anthropic_version: Option<String>,
    pub messages_auth: MessagesAuth,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderPaths {
    pub openai_chat: Option<String>,
    pub openai_responses: Option<String>,
    pub anthropic_messages: Option<String>,
    pub gemini: Option<String>,
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
            Protocol::Gemini => paths.gemini = Some(protocol.upstream_path().into()),
        }
        paths
    }

    pub fn get(&self, protocol: Protocol) -> Option<&str> {
        match protocol {
            Protocol::OpenAiChat => self.openai_chat.as_deref(),
            Protocol::OpenAiResponses => self.openai_responses.as_deref(),
            Protocol::AnthropicMessages => self.anthropic_messages.as_deref(),
            Protocol::Gemini => self.gemini.as_deref(),
        }
    }

    pub fn supported(&self) -> Vec<Protocol> {
        [
            Protocol::OpenAiChat,
            Protocol::OpenAiResponses,
            Protocol::AnthropicMessages,
            Protocol::Gemini,
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
    pub messages_auth: MessagesAuth,
}

#[derive(Clone, Debug)]
pub struct ModelPrice {
    pub input_per_million: String,
    pub output_per_million: String,
}

#[derive(Clone, Debug)]
pub struct ModelMappingInput {
    /// 实际 Provider／模型的思考能力与启用参数。
    pub thinking: llmproxy_core::thinking::Config,
    pub alias: String,
    pub provider_id: i64,
    pub upstream_model_id: String,
    pub protocols: Vec<Protocol>,
    pub reference_price: Option<ModelPrice>,
}

#[derive(Clone, Debug)]
pub struct ModelMappingView {
    pub group_ids: Vec<i64>,
    /// 实际 Provider／模型的思考能力与启用参数。
    pub thinking: llmproxy_core::thinking::Config,
    pub id: i64,
    pub alias: String,
    pub provider_id: i64,
    pub provider_name: String,
    pub upstream_model_id: String,
    pub protocols: Vec<Protocol>,
    pub reference_price: Option<ModelPrice>,
    pub provider_enabled: bool,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct ModelRouteTargetInput {
    pub model_id: i64,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ModelRouteInput {
    pub name: String,
    pub protocol: Protocol,
    pub provider_protocol: Protocol,
    pub enabled: bool,
    pub targets: Vec<ModelRouteTargetInput>,
}

#[derive(Clone, Debug)]
pub struct ModelRouteTargetView {
    pub model: ModelMappingView,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ModelRouteView {
    pub group_ids: Vec<i64>,
    pub id: i64,
    pub name: String,
    pub protocol: Protocol,
    pub provider_protocol: Protocol,
    pub enabled: bool,
    pub targets: Vec<ModelRouteTargetView>,
    pub version: u64,
}

/// Server-only model route. Credentials are never serialized to the browser.
pub struct ModelRoute {
    /// 最终选择的模型映射；没有启用目标时为空。
    pub model_id: Option<i64>,
    /// 实际 Provider／模型的思考能力与启用参数。
    pub thinking: llmproxy_core::thinking::Config,
    pub alias: String,
    pub upstream_model_id: String,
    pub enabled: bool,
    pub provider: Option<ActiveProvider>,
    pub protocol: Protocol,
}

#[derive(Clone, Debug)]
pub struct GroupView {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct VirtualKeyInput {
    pub name: String,
    pub all_routes: bool,
    pub route_ids: Vec<i64>,
    pub model_ids: Vec<i64>,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct VirtualKeyView {
    pub id: i64,
    pub group_id: i64,
    pub name: String,
    pub prefix: String,
    pub all_routes: bool,
    pub route_ids: Vec<i64>,
    pub model_ids: Vec<i64>,
    pub enabled: bool,
    pub revoked: bool,
    pub expires_at: Option<i64>,
    pub version: u64,
}

/// 一次性返回的明文密钥，不实现 Debug/Serialize。
pub struct CreatedVirtualKey {
    pub view: VirtualKeyView,
    pub secret: String,
}

#[derive(Clone, Debug)]
pub struct CallIdentity {
    pub group_id: i64,
    pub key_id: i64,
    pub all_routes: bool,
    pub route_ids: Vec<i64>,
    pub model_ids: Vec<i64>,
}
