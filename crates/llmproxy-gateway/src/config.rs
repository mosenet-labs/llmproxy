use std::{env, error::Error, net::SocketAddr};

pub struct Settings {
    pub listen: SocketAddr,
    pub database_url: String,
    pub master_key: String,
}

pub fn load() -> Result<Settings, Box<dyn Error + Send + Sync>> {
    let database_url =
        env::var("LLMPROXY_DATABASE_URL").map_err(|_| "LLMPROXY_DATABASE_URL is required")?;
    if database_url.trim().is_empty() {
        return Err("LLMPROXY_DATABASE_URL must not be empty".into());
    }
    let master_key =
        env::var("LLMPROXY_MASTER_KEY").map_err(|_| "LLMPROXY_MASTER_KEY is required")?;
    let listen = env::var("LLMPROXY_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:3200".to_owned())
        .parse::<SocketAddr>()
        .map_err(|_| "LLMPROXY_LISTEN must be an IP socket address")?;
    if listen.port() == 0 {
        return Err("LLMPROXY_LISTEN port must be greater than zero".into());
    }
    Ok(Settings {
        listen,
        database_url,
        master_key,
    })
}
