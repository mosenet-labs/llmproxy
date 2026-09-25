use llmproxy_core::protocol::Protocol;

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
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub encrypted_key: String,
    pub enabled: bool,
    pub models_path: String,
    pub models_protocol: String,
    pub models_probe_status: String,
    pub anthropic_version: Option<String>,
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
#[table = "store_keys"]
pub(crate) struct StoreKey {
    #[key]
    pub id: i64,
    pub encrypted_verifier: String,
}

pub(crate) fn protocol(value: &str) -> StoreResult<Protocol> {
    match value {
        "openai_chat" => Ok(Protocol::OpenAiChat),
        "openai_responses" => Ok(Protocol::OpenAiResponses),
        "anthropic_messages" => Ok(Protocol::AnthropicMessages),
        _ => Err(StoreError::Internal),
    }
}

impl Provider {
    pub fn paths(&self) -> ProviderPaths {
        ProviderPaths {
            openai_chat: self.openai_chat_path.clone(),
            openai_responses: self.openai_responses_path.clone(),
            anthropic_messages: self.anthropic_messages_path.clone(),
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
