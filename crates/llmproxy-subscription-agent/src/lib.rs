//! 独立订阅代理；诊断能力存在与运行验收分别记录。

pub mod codex_rpc;
pub mod credentials;
pub mod doctor;
pub mod http;
pub mod native;
pub mod oauth;
pub mod responses;
pub mod reverse;
pub mod service;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub fn random_id() -> Result<String, Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "无法生成随机标识")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn now() -> Result<u64, Error> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}
