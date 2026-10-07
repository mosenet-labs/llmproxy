use llmproxy_core::protocol::{MessagesAuth, Protocol};

use crate::{ProbeStatus, ProviderPaths, ProviderView, StoreError, StoreResult};

#[derive(toasty::Model)]
#[table = "providers"]
pub(crate) struct Provider {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub name: String,
    pub openai_chat_path: Option<String>,
    pub openai_responses_path: Option<String>,
    pub anthropic_messages_path: Option<String>,
    pub gemini_path: Option<String>,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub encrypted_key: String,
    pub enabled: bool,
    pub models_path: String,
    pub models_protocol: String,
    pub models_probe_status: String,
    pub anthropic_version: Option<String>,
    pub messages_auth: String,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    #[version]
    pub version: u64,
    pub updated_at: i64,
}

#[derive(toasty::Model)]
#[table = "route_bindings"]
pub(crate) struct RouteBinding {
    #[key]
    pub protocol: String,
    pub provider_id: Option<i64>,
    #[version]
    pub version: u64,
}

#[derive(toasty::Model)]
#[table = "model_mappings"]
pub(crate) struct ModelMapping {
    /// 类型化思考配置的序列化文本；空值兼容旧模型。
    pub thinking_json: Option<String>,
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub alias: String,
    pub provider_id: i64,
    pub upstream_model_id: String,
    pub openai_chat: bool,
    pub openai_responses: bool,
    pub anthropic_messages: bool,
    pub gemini: bool,
    pub input_price_per_million: Option<String>,
    pub output_price_per_million: Option<String>,
    #[version]
    pub version: u64,
    pub updated_at: i64,
}

#[derive(toasty::Model)]
#[table = "model_routes"]
pub(crate) struct ModelRouteRow {
    #[key]
    #[auto]
    pub id: i64,
    pub name: String,
    pub protocol: String,
    pub provider_protocol: String,
    pub enabled: bool,
    #[version]
    pub version: u64,
    pub updated_at: i64,
}

#[derive(toasty::Model)]
#[table = "model_route_targets"]
pub(crate) struct ModelRouteTargetRow {
    #[key]
    #[auto]
    pub id: i64,
    pub route_id: i64,
    pub model_id: i64,
    pub position: i64,
    pub enabled: bool,
}

#[derive(toasty::Model)]
#[table = "model_price_plans"]
pub(crate) struct ModelPricePlan {
    #[key]
    #[auto]
    pub id: i64,
    pub provider_id: i64,
    pub upstream_model_id: String,
    pub currency: String,
    pub source_kind: String,
    pub source_url: Option<String>,
    pub recorded_at: i64,
    pub effective_at: Option<i64>,
    pub is_current: bool,
    pub schedule_json: Option<String>,
}

#[derive(toasty::Model)]
#[table = "model_price_rules"]
pub(crate) struct ModelPriceRule {
    #[key]
    #[auto]
    pub id: i64,
    pub price_plan_id: i64,
    pub item_code: String,
    pub unit_code: String,
    pub unit_size: i64,
    pub conditions_json: String,
    pub unit_price: String,
}

#[derive(toasty::Model)]
#[table = "holiday_dates"]
pub(crate) struct HolidayDateRow {
    #[key]
    pub date: String,
    pub year: i64,
    pub name: String,
    pub kind: String,
    pub source_url: String,
    pub imported_at: i64,
}

#[derive(toasty::Model)]
#[table = "store_keys"]
pub(crate) struct StoreKey {
    #[key]
    pub id: i64,
    pub encrypted_verifier: String,
}

/// 工具续接状态只保存密文，原始调用参数和签名不暴露给控制台。
#[derive(toasty::Model)]
#[table = "tool_continuations"]
pub(crate) struct ToolContinuationRow {
    #[key]
    pub id: String,
    pub scope: String,
    pub encrypted_payload: String,
    pub payload_bytes: i64,
    pub expires_at: i64,
}

pub(crate) fn protocol(value: &str) -> StoreResult<Protocol> {
    match value {
        "openai_chat" => Ok(Protocol::OpenAiChat),
        "openai_responses" => Ok(Protocol::OpenAiResponses),
        "anthropic_messages" => Ok(Protocol::AnthropicMessages),
        "gemini" => Ok(Protocol::Gemini),
        _ => Err(StoreError::Internal),
    }
}

impl Provider {
    pub fn paths(&self) -> ProviderPaths {
        ProviderPaths {
            openai_chat: self.openai_chat_path.clone(),
            openai_responses: self.openai_responses_path.clone(),
            anthropic_messages: self.anthropic_messages_path.clone(),
            gemini: self.gemini_path.clone(),
        }
    }

    pub fn view(&self, active_protocols: Vec<Protocol>) -> StoreResult<ProviderView> {
        Ok(ProviderView {
            id: self.id,
            name: self.name.clone(),
            paths: self.paths(),
            host: self.host.clone(),
            port: self.port,
            tls: self.tls,
            enabled: self.enabled,
            models_path: self.models_path.clone(),
            models_protocol: protocol(&self.models_protocol)?,
            models_probe_status: ProbeStatus::parse(&self.models_probe_status)?,
            anthropic_version: self.anthropic_version.clone(),
            messages_auth: MessagesAuth::parse(&self.messages_auth).ok_or(StoreError::Internal)?,
            connect_timeout_ms: self.connect_timeout_ms,
            read_timeout_ms: self.read_timeout_ms,
            write_timeout_ms: self.write_timeout_ms,
            active: !active_protocols.is_empty(),
            active_protocols,
            version: self.version,
            key_configured: !self.encrypted_key.is_empty(),
            updated_at: self.updated_at,
        })
    }
}

impl ModelMapping {
    /// 配置损坏时拒绝读取，不能回退为可关闭。
    pub fn thinking(&self) -> StoreResult<llmproxy_core::thinking::Config> {
        let config: llmproxy_core::thinking::Config = self
            .thinking_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| StoreError::Internal)?
            .unwrap_or_default();
        config
            .validate(&self.protocols())
            .map_err(|_| StoreError::Internal)?;
        Ok(config)
    }

    pub fn protocols(&self) -> Vec<Protocol> {
        [
            (self.openai_chat, Protocol::OpenAiChat),
            (self.openai_responses, Protocol::OpenAiResponses),
            (self.anthropic_messages, Protocol::AnthropicMessages),
            (self.gemini, Protocol::Gemini),
        ]
        .into_iter()
        .filter_map(|(enabled, protocol)| enabled.then_some(protocol))
        .collect()
    }
}

/// 会话与轮次独立建表，模型配置删除不级联删除历史。
#[derive(toasty::Model)]
#[table = "chat_conversations"]
pub(crate) struct ChatConversationRow {
    #[key]
    pub id: String,
    pub owner: String,
    pub title: String,
    pub selection_json: String,
    pub thinking: String,
    pub active_turn: Option<String>,
    /// 归档保留内容与统计，仅从默认历史列表隐藏。
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
    #[version]
    pub version: u64,
}
#[derive(toasty::Model)]
#[table = "chat_turns"]
pub(crate) struct ChatTurnRow {
    #[key]
    pub id: String,
    pub conversation_id: String,
    pub sequence: i64,
    pub selection_json: String,
    pub alias: String,
    pub thinking: String,
    pub encrypted_prompt: String,
    pub encrypted_reply: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub actual_json: Option<String>,
    pub usage_json: Option<String>,
    pub usage_state: String,
    pub gateway_started: bool,
    pub gateway_finished: bool,
    pub provider_id: Option<i64>,
    pub upstream_model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}
