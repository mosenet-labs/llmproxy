use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use llmproxy_core::{
    protocol::Protocol,
    routing::{Route, match_route},
};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use pingora::{
    Result,
    proxy::{ProxyHttp, Session},
    upstreams::peer::HttpPeer,
};
use pingora_http::RequestHeader;
use tracing::Span;

#[derive(Clone)]
pub struct ResolvedConfig {
    pub listen: String,
    pub openai_chat: ResolvedProvider,
    pub openai_responses: ResolvedProvider,
    pub anthropic_messages: ResolvedProvider,
}

#[derive(Clone)]
pub struct ResolvedProvider {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub secret: String,
    pub anthropic_version: Option<String>,
}

impl ResolvedConfig {
    fn provider(&self, protocol: Protocol) -> &ResolvedProvider {
        match protocol {
            Protocol::OpenAiChat => &self.openai_chat,
            Protocol::OpenAiResponses => &self.openai_responses,
            Protocol::AnthropicMessages => &self.anthropic_messages,
        }
    }
}

pub struct Gateway {
    config: Arc<ResolvedConfig>,
    requests: Counter<u64>,
    failures: Counter<u64>,
    duration: Histogram<f64>,
}

pub struct RequestContext {
    start: Instant,
    protocol: Option<Protocol>,
    span: Option<Span>,
}

impl Gateway {
    pub fn new(config: Arc<ResolvedConfig>) -> Self {
        let meter = opentelemetry::global::meter("llmproxy-gateway");
        Self {
            config,
            requests: meter.u64_counter("llmproxy.requests").build(),
            failures: meter.u64_counter("llmproxy.failures").build(),
            duration: meter
                .f64_histogram("llmproxy.duration")
                .with_unit("s")
                .build(),
        }
    }
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = RequestContext;

    fn new_ctx(&self) -> Self::CTX {
        RequestContext {
            start: Instant::now(),
            protocol: None,
            span: None,
        }
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let request = session.req_header();
        let method = request.method.as_str();
        let path = request.uri.path();
        let route = match_route(method, path);
        let span = tracing::info_span!(
            "llmproxy.request",
            http.method = method,
            http.route = path,
            llm.protocol = tracing::field::Empty,
            http.status_code = tracing::field::Empty,
        );
        ctx.span = Some(span);

        match route {
            Route::Proxy(protocol) => {
                ctx.protocol = Some(protocol);
                if let Some(span) = &ctx.span {
                    span.record("llm.protocol", protocol.as_str());
                }
                Ok(false)
            }
            Route::Auto => {
                session.respond_error(501).await?;
                Ok(true)
            }
            Route::MethodNotAllowed => {
                session.respond_error(405).await?;
                Ok(true)
            }
            Route::NotFound => {
                session.respond_error(404).await?;
                Ok(true)
            }
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let protocol = ctx.protocol.expect("request_filter set protocol");
        let provider = self.config.provider(protocol);
        let peer = HttpPeer::new(
            (provider.host.as_str(), provider.port),
            provider.tls,
            provider.host.clone(),
        );
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        let protocol = ctx.protocol.expect("request_filter set protocol");
        let provider = self.config.provider(protocol);
        request.remove_header("authorization");
        request.remove_header("x-api-key");
        request.insert_header("host", provider.host.as_str())?;
        match protocol {
            Protocol::AnthropicMessages => {
                request.insert_header("x-api-key", provider.secret.as_str())?;
                if let Some(version) = &provider.anthropic_version {
                    request.insert_header("anthropic-version", version.as_str())?;
                }
            }
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                request.insert_header("authorization", format!("Bearer {}", provider.secret))?;
            }
        }
        Ok(())
    }

    async fn logging(
        &self,
        session: &mut Session,
        error: Option<&pingora::Error>,
        ctx: &mut Self::CTX,
    ) {
        let status = session
            .response_written()
            .map(|header| header.status.as_u16())
            .unwrap_or(502);
        let protocol = ctx.protocol.map(Protocol::as_str).unwrap_or("none");
        let attributes = [
            KeyValue::new("protocol", protocol),
            KeyValue::new("status", status as i64),
        ];
        self.requests.add(1, &attributes);
        if error.is_some() || status >= 500 {
            self.failures.add(1, &attributes);
        }
        let duration = ctx.start.elapsed().as_secs_f64();
        self.duration.record(duration, &attributes);
        if let Some(span) = &ctx.span {
            span.record("http.status_code", status);
            let _entered = span.enter();
            tracing::info!(
                status,
                duration_seconds = duration,
                protocol,
                failed = error.is_some(),
                "request completed"
            );
        }
    }
}
