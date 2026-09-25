mod console;
mod observability;
mod proxy;
mod snapshot;

use std::{env, error::Error, fs};

use llmproxy_core::{config::GatewayConfig, protocol::Protocol};
use pingora::{proxy::http_proxy_service, server::Server};

use crate::{
    proxy::Gateway,
    snapshot::{ProviderSnapshot, ProviderSnapshots, ResolvedProvider},
};

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    dotenvy::dotenv().ok();
    let _telemetry = llmproxy_telemetry::init()?;
    let mut _console_runtime = None;
    let (listen, providers, _refresh, console) = match env::var("LLMPROXY_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => {
            let master_key = env::var("LLMPROXY_MASTER_KEY")
                .map_err(|_| "LLMPROXY_MASTER_KEY is required in database mode")?;
            let listen =
                env::var("LLMPROXY_LISTEN").unwrap_or_else(|_| "127.0.0.1:3200".to_owned());
            let address = listen
                .parse::<std::net::SocketAddr>()
                .map_err(|_| "LLMPROXY_LISTEN must be an IP socket address")?;
            if address.port() == 0 {
                return Err("LLMPROXY_LISTEN port must be greater than zero".into());
            }
            let (providers, refresh) = ProviderSnapshots::database(&url, &master_key)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            let console = runtime.block_on(llmproxy_console::Console::connect(
                &url,
                &master_key,
                address.port(),
            ))?;
            _console_runtime = Some(runtime);
            (listen, providers, Some(refresh), Some(console))
        }
        Ok(_) | Err(env::VarError::NotPresent) => {
            let (listen, providers) = load_file_config()?;
            (listen, providers, None, None)
        }
        Err(env::VarError::NotUnicode(_)) => {
            return Err("LLMPROXY_DATABASE_URL must contain valid Unicode".into());
        }
    };
    let mut server = Server::new(None)?;
    server.bootstrap();
    let mut service = http_proxy_service(&server.configuration, Gateway::new(providers, console));
    service.add_tcp(&listen);
    observability::listening(&listen);
    server.add_service(service);
    // run_forever exits the process directly, skipping the telemetry guard.
    server.run(Default::default());
    Ok(())
}

fn load_file_config() -> Result<(String, ProviderSnapshots), Box<dyn Error + Send + Sync>> {
    let config_path =
        env::var("LLMPROXY_CONFIG").unwrap_or_else(|_| "config/gateway.example.toml".to_owned());
    let config: GatewayConfig = toml::from_str(&fs::read_to_string(&config_path)?)?;
    config.validate()?;

    let snapshot = ProviderSnapshot::new([
        (
            Protocol::OpenAiChat,
            resolve(&config, Protocol::OpenAiChat)?,
        ),
        (
            Protocol::OpenAiResponses,
            resolve(&config, Protocol::OpenAiResponses)?,
        ),
        (
            Protocol::AnthropicMessages,
            resolve(&config, Protocol::AnthropicMessages)?,
        ),
    ])?;
    Ok((config.listen, ProviderSnapshots::new(snapshot)))
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
        connect_timeout_ms: provider.connect_timeout_ms,
        read_timeout_ms: provider.read_timeout_ms,
        write_timeout_ms: provider.write_timeout_ms,
    })
}
