use llmproxy_store::DatabaseConfig;
use std::{env, error::Error, net::SocketAddr};

pub struct Settings {
    pub listen: SocketAddr,
    pub database: DatabaseConfig,
}

pub fn load() -> Result<Settings, Box<dyn Error + Send + Sync>> {
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
