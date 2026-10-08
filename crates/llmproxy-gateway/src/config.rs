use llmproxy_store::DatabaseConfig;
use std::{env, error::Error, net::SocketAddr};

pub struct Settings {
    pub listen: SocketAddr,
    pub database: DatabaseConfig,
}

pub fn load() -> Result<Settings, Box<dyn Error + Send + Sync>> {
    let registration = env::var("LLMPROXY_SUBSCRIPTION_REGISTRATION_KEY").ok();
    let relay = env::var("LLMPROXY_SUBSCRIPTION_RELAY_KEY").ok();
    if registration.is_some() != relay.is_some()
        || registration.as_ref().is_some_and(|key| key.len() < 32)
        || relay.as_ref().is_some_and(|key| key.len() < 32)
        || registration
            .as_ref()
            .zip(relay.as_ref())
            .is_some_and(|(a, b)| a == b)
    {
        return Err("订阅节点需要分别配置至少 32 字符且不同的注册与转接密钥".into());
    }
    let database = DatabaseConfig::from_env()?;
    let listen = env::var("LLMPROXY_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:3200".to_owned())
        .parse::<SocketAddr>()
        .map_err(|_| "LLMPROXY_LISTEN must be an IP socket address")?;
    if listen.port() == 0 {
        return Err("LLMPROXY_LISTEN port must be greater than zero".into());
    }
    Ok(Settings { listen, database })
}
