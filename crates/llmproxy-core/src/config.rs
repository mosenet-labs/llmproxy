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
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_read_timeout_ms")]
    pub read_timeout_ms: u64,
    #[serde(default = "default_write_timeout_ms")]
    pub write_timeout_ms: u64,
}

const fn default_connect_timeout_ms() -> u64 {
    10_000
}

const fn default_read_timeout_ms() -> u64 {
    60_000
}

const fn default_write_timeout_ms() -> u64 {
    30_000
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
        if self.connect_timeout_ms == 0 {
            return Err("connect_timeout_ms must be nonzero");
        }
        if self.read_timeout_ms == 0 {
            return Err("read_timeout_ms must be nonzero");
        }
        if self.write_timeout_ms == 0 {
            return Err("write_timeout_ms must be nonzero");
        }
        if let Some(version) = &self.anthropic_version {
            // Version identifiers are ASCII; reject controls and non-ASCII bytes at
            // startup so inserting the configured HTTP header cannot fail later.
            if version.trim().is_empty()
                || !version
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() || byte == b' ')
            {
                return Err("anthropic_version must be a nonempty printable ASCII header value");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{GatewayConfig, ProviderConfig};
    use serde_json::{Value, json};

    fn provider_json() -> Value {
        json!({
            "host": "api.example.com",
            "port": 443,
            "tls": true,
            "api_key_env": "PROVIDER_API_KEY"
        })
    }

    #[test]
    fn existing_config_uses_default_timeouts() {
        let config: GatewayConfig = serde_json::from_value(json!({
            "listen": "127.0.0.1:8080",
            "openai_chat": provider_json(),
            "openai_responses": provider_json(),
            "anthropic_messages": provider_json()
        }))
        .unwrap();

        config.validate().unwrap();
        for provider in [
            config.openai_chat,
            config.openai_responses,
            config.anthropic_messages,
        ] {
            assert_eq!(provider.connect_timeout_ms, 10_000);
            assert_eq!(provider.read_timeout_ms, 60_000);
            assert_eq!(provider.write_timeout_ms, 30_000);
        }
    }

    #[test]
    fn provider_can_override_each_timeout() {
        let mut config = provider_json();
        config["connect_timeout_ms"] = json!(125);
        config["read_timeout_ms"] = json!(250);
        config["write_timeout_ms"] = json!(500);
        let provider: ProviderConfig = serde_json::from_value(config).unwrap();

        provider.validate().unwrap();
        assert_eq!(provider.connect_timeout_ms, 125);
        assert_eq!(provider.read_timeout_ms, 250);
        assert_eq!(provider.write_timeout_ms, 500);
    }

    #[test]
    fn zero_timeout_is_rejected() {
        for field in ["connect_timeout_ms", "read_timeout_ms", "write_timeout_ms"] {
            let mut config = provider_json();
            config[field] = json!(0);
            let provider: ProviderConfig = serde_json::from_value(config).unwrap();
            assert!(provider.validate().unwrap_err().starts_with(field));
        }
    }

    #[test]
    fn configured_anthropic_version_must_be_a_safe_header_value() {
        for version in [
            "",
            "  ",
            "2023-06-01\r\nx-header: value",
            "\t",
            "\0",
            "版本",
            "\u{7f}",
        ] {
            let mut config = provider_json();
            config["anthropic_version"] = json!(version);
            let provider: ProviderConfig = serde_json::from_value(config).unwrap();
            assert!(provider.validate().is_err(), "accepted {version:?}");
        }

        let mut config = provider_json();
        config["anthropic_version"] = json!("2023-06-01");
        let provider: ProviderConfig = serde_json::from_value(config).unwrap();
        provider.validate().unwrap();
    }
}
