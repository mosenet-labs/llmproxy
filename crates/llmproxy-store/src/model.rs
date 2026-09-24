use llmproxy_core::protocol::Protocol;

use crate::{ProviderView, StoreError, StoreResult};

#[derive(toasty::Model)]
#[table = "providers"]
pub(crate) struct Provider {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub name: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub encrypted_key: String,
    pub enabled: bool,
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
    pub fn view(&self, active: bool) -> StoreResult<ProviderView> {
        Ok(ProviderView {
            id: self.id,
            name: self.name.clone(),
            protocol: protocol(&self.protocol)?,
            host: self.host.clone(),
            port: self.port,
            tls: self.tls,
            enabled: self.enabled,
            anthropic_version: self.anthropic_version.clone(),
            connect_timeout_ms: self.connect_timeout_ms,
            read_timeout_ms: self.read_timeout_ms,
            write_timeout_ms: self.write_timeout_ms,
            active,
            version: self.version,
            key_configured: !self.encrypted_key.is_empty(),
            updated_at: self.updated_at,
        })
    }
}
