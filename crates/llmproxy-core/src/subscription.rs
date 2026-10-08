//! 订阅节点反向协议；不含 OAuth 凭据，不提供任意 URL 或 RPC 隧道。
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
pub struct Registration {
    pub version: u32,
    pub node_id: String,
    pub node_key: String,
    #[serde(default)]
    pub name: String,
    pub backend: String,
    pub models: Vec<String>,
    pub concurrency: usize,
    pub health: Health,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Ready,
    Abnormal,
    Unknown,
}

#[derive(Serialize, Deserialize)]
pub struct Lease {
    pub token: String,
    pub heartbeat_seconds: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Work {
    pub request_id: String,
    pub body: Value,
}

#[derive(Serialize, Deserialize)]
pub struct Heartbeat {
    pub health: Health,
}

#[derive(Clone, Debug)]
pub struct Presence {
    pub online_until: u64,
    pub health: Health,
}

#[derive(Serialize, Deserialize)]
pub struct Cancellations {
    pub request_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResultFrame {
    Head { status: u16, content_type: String },
    Data { base64: String },
    End,
    Error,
}
