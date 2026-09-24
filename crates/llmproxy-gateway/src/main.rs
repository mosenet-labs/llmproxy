mod observability;
mod proxy;

use std::{env, error::Error, fs, sync::Arc};

use llmproxy_core::{config::GatewayConfig, protocol::Protocol};
use pingora::{proxy::http_proxy_service, server::Server};

use crate::proxy::{Gateway, ResolvedConfig, ResolvedProvider};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let config_path =
        env::var("LLMPROXY_CONFIG").unwrap_or_else(|_| "config/gateway.example.toml".to_owned());
    let config: GatewayConfig = toml::from_str(&fs::read_to_string(&config_path)?)?;
    config.validate()?;

    let resolved = ResolvedConfig {
        listen: config.listen.clone(),
        openai_chat: resolve(&config, Protocol::OpenAiChat)?,
        openai_responses: resolve(&config, Protocol::OpenAiResponses)?,
        anthropic_messages: resolve(&config, Protocol::AnthropicMessages)?,
    };

    let _telemetry = observability::init()?;
    let mut server = Server::new(None)?;
    server.bootstrap();
    let mut service = http_proxy_service(
        &server.configuration,
        Gateway::new(Arc::new(resolved.clone())),
    );
    service.add_tcp(&resolved.listen);
    tracing::info!(listen = %resolved.listen, "gateway listening");
    server.add_service(service);
    server.run_forever();
}

fn resolve(
    config: &GatewayConfig,
    protocol: Protocol,
) -> Result<ResolvedProvider, Box<dyn Error + Send + Sync>> {
    let provider = config.provider(protocol);
    let secret = env::var(&provider.api_key_env)
        .map_err(|_| format!("missing environment variable {}", provider.api_key_env))?;
    if secret.trim().is_empty() || secret.contains(['\r', '\n']) {
        return Err(format!("{} is empty or contains a line break", provider.api_key_env).into());
    }
    Ok(ResolvedProvider {
        host: provider.host.clone(),
        port: provider.port,
        tls: provider.tls,
        secret,
        anthropic_version: provider.anthropic_version.clone(),
    })
}
