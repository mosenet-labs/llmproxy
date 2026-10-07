//! 选路完成后的必需信息共同保存，响应外壳在本次请求内保持稳定。
use crate::snapshot::ResolvedProvider;
use llmproxy_core::{adapter::protocol_codec::ResponseTarget, protocol::Protocol};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static RESPONSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(super) struct SelectedRoute {
    pub thinking: llmproxy_core::thinking::Config,
    pub protocol: Protocol,
    pub client_model: String,
    pub provider: Arc<ResolvedProvider>,
    pub upstream_model: String,
    pub stream: bool,
    response: Option<ResponseShell>,
}

#[derive(Clone)]
struct ResponseShell {
    id: String,
    created: i64,
}

impl SelectedRoute {
    /// 固定 Provider 快照；仅跨协议请求需要网关生成响应 ID 和创建时间。
    pub fn new(
        protocol: Protocol,
        client_model: String,
        provider: Arc<ResolvedProvider>,
        upstream_model: String,
        stream: bool,
        thinking: llmproxy_core::thinking::Config,
    ) -> Self {
        let response = (protocol != provider.protocol).then(|| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let sequence = RESPONSE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            ResponseShell {
                id: format!("llmproxy-{}-{sequence}", now.as_nanos()),
                created: now.as_secs() as i64,
            }
        });
        Self {
            thinking,
            protocol,
            client_model,
            provider,
            upstream_model,
            stream,
            response,
        }
    }

    /// 路由已经固定后才能决定是否进入跨协议子请求。
    pub fn is_cross_protocol(&self) -> bool {
        self.protocol != self.provider.protocol
    }

    /// 非流式与流式共用同一个客户端响应外壳。
    pub fn response_target(&self) -> ResponseTarget<'_> {
        let response = self
            .response
            .as_ref()
            .expect("cross-protocol response shell");
        ResponseTarget {
            model: &self.client_model,
            id: &response.id,
            created: response.created,
        }
    }
}
