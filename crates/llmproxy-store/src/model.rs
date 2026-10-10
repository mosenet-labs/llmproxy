use llmproxy_core::protocol::{MessagesAuth, Protocol};

use crate::{ProbeStatus, ProviderPaths, ProviderView, StoreError, StoreResult};

#[derive(toasty::Model)]
#[table = "mail_settings"]
pub(crate) struct MailSettingsRow {
    #[key]
    pub id: i64,
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub tls: String,
    #[column("from_email")]
    pub sender_email: String,
    pub username: String,
    pub encrypted_password: String,
    #[version]
    pub version: u64,
}

#[derive(toasty::Model)]
#[table = "users"]
pub(crate) struct UserRow {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub email: String,
    pub display_name: String,
    pub password_hash: String,
    pub role: String,
    pub enabled: bool,
    pub email_verified: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_login_at: Option<i64>,
    #[version]
    pub version: u64,
}

#[derive(toasty::Model)]
#[table = "user_sessions"]
pub(crate) struct UserSessionRow {
    #[key]
    pub digest: String,
    pub user_id: i64,
    pub csrf: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
}

#[derive(toasty::Model)]
#[table = "email_challenges"]
pub(crate) struct EmailChallengeRow {
    #[key]
    pub scope: String,
    pub encrypted_code: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub attempts: u64,
    pub used: bool,
}

#[derive(toasty::Model)]
#[table = "subscription_nodes"]
pub(crate) struct SubscriptionNode {
    #[key]
    pub node_id: String,
    pub name: String,
    pub provider_name: Option<String>,
    pub config_version: u64,
    pub backend: String,
    #[column(type = text)]
    pub models_json: toasty::Json<Vec<String>>,
    pub concurrency: u64,
    pub encrypted_node_key: String,
    pub enabled: bool,
    pub provider_id: Option<i64>,
    #[version]
    pub version: u64,
    pub updated_at: i64,
}

#[derive(toasty::Model)]
#[table = "providers"]
pub(crate) struct Provider {
    #[key]
    #[auto]
    pub id: i64,
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
    /// 沿用 TEXT 存储和 SQL NULL，兼容旧模型。
    #[column(type = text)]
    pub thinking_json: Option<toasty::Json<llmproxy_core::thinking::Config>>,
    #[key]
    #[auto]
    pub id: i64,
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
    #[column(type = text)]
    pub schedule_json: Option<toasty::Json<crate::PriceSchedule>>,
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
    #[column(type = text)]
    pub conditions_json: toasty::Json<crate::PriceConditions>,
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
            .as_ref()
            .map(|config| config.0.clone())
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

#[derive(toasty::Model)]
#[table = "model_health_checks"]
pub(crate) struct ModelHealthCheck {
    #[key]
    #[auto]
    pub id: i64,
    pub model_id: i64,
    pub protocol: String,
    pub enabled: bool,
    pub interval_seconds: u64,
    pub timeout_ms: u64,
    pub max_output_tokens: u32,
    pub next_probe_at: i64,
    pub lease_until: i64,
    pub generation: u64,
    pub model_version: u64,
    pub provider_version: u64,
    pub last_probe_at: Option<i64>,
    pub last_success_at: Option<i64>,
    #[column(type = text)]
    pub result_json: Option<toasty::Json<llmproxy_probe::ProbeResult>>,
    pub consecutive_failures: u64,
    #[version]
    pub version: u64,
}

#[derive(toasty::Model)]
#[table = "groups"]
pub(crate) struct GroupRow {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub name: String,
    pub enabled: bool,
    #[version]
    pub version: u64,
    pub updated_at: i64,
}

#[derive(toasty::Model)]
#[table = "virtual_keys"]
pub(crate) struct VirtualKeyRow {
    #[key]
    #[auto]
    pub id: i64,
    pub group_id: i64,
    pub name: String,
    #[unique]
    pub digest: String,
    pub prefix: String,
    pub all_routes: bool,
    #[column(type = text)]
    pub route_ids: toasty::Json<Vec<i64>>,
    #[column(type = text)]
    pub model_ids: toasty::Json<Vec<i64>>,
    pub enabled: bool,
    pub revoked: bool,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    #[version]
    pub version: u64,
}

#[derive(toasty::Model)]
#[table = "model_group_memberships"]
pub(crate) struct ModelGroupMembership {
    #[key]
    #[auto]
    pub id: i64,
    pub group_id: i64,
    pub model_id: i64,
}

#[derive(toasty::Model)]
#[table = "route_group_memberships"]
pub(crate) struct RouteGroupMembership {
    #[key]
    #[auto]
    pub id: i64,
    pub group_id: i64,
    pub route_id: i64,
}
