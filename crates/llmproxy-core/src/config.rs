use serde::{Deserialize, Serialize};

use crate::protocol::Protocol;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    pub listen: String,
    pub openai_chat: ProviderConfig,
    pub openai_responses: ProviderConfig,
    pub anthropic_messages: ProviderConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub api_key_env: String,
    pub anthropic_version: Option<String>,
}

impl GatewayConfig {
    pub fn provider(&self, protocol: Protocol) -> &ProviderConfig {
        match protocol {
            Protocol::OpenAiChat => &self.openai_chat,
            Protocol::OpenAiResponses => &self.openai_responses,
            Protocol::AnthropicMessages => &self.anthropic_messages,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.listen.parse::<std::net::SocketAddr>().is_err() {
            return Err("listen must be an IP socket address".into());
        }
        for protocol in [
            Protocol::OpenAiChat,
            Protocol::OpenAiResponses,
            Protocol::AnthropicMessages,
        ] {
            self.provider(protocol)
                .validate()
                .map_err(|error| format!("{}: {error}", protocol.as_str()))?;
        }
        Ok(())
    }
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host.is_empty()
            || self.host.contains('/')
            || self.host.contains(':')
            || self.host.chars().any(char::is_whitespace)
        {
            return Err("host must be a DNS name or IPv4 address without scheme or path");
        }
        if self.port == 0 {
            return Err("port must be nonzero");
        }
        if self.api_key_env.is_empty()
            || !self
                .api_key_env
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err("api_key_env must contain uppercase letters, digits or underscore");
        }
        Ok(())
    }
}
