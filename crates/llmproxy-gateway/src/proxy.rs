use std::{
    net::ToSocketAddrs,
    sync::Arc,
    time::{Duration, Instant},
};

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
    Error, ErrorSource, ErrorType, Result,
    proxy::{FailToProxy, ProxyHttp, Session},
    upstreams::peer::HttpPeer,
};
use pingora_http::{RequestHeader, ResponseHeader};
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
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

impl ResolvedProvider {
    fn authority(&self) -> String {
        let default_port = if self.tls { 443 } else { 80 };
        if self.port == default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
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
                session.set_keepalive(None);
                let mut response = ResponseHeader::build(405, Some(2))?;
                response.insert_header("allow", "POST")?;
                response.insert_header("content-length", "0")?;
                session
                    .write_response_header(Box::new(response), true)
                    .await?;
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
        // HttpPeer::new unwraps DNS errors when passed a hostname. Resolve here
        // so a lookup failure follows the normal upstream error path.
        let address = (provider.host.as_str(), provider.port)
            .to_socket_addrs()
            .map_err(|error| {
                Error::because(ErrorType::ConnectError, "provider DNS lookup failed", error)
                    .into_up()
            })?
            .next()
            .ok_or_else(|| {
                Error::explain(
                    ErrorType::ConnectError,
                    "provider DNS lookup returned no addresses",
                )
                .into_up()
            })?;
        let mut peer = HttpPeer::new(address, provider.tls, provider.host.clone());
        let connect_timeout = Duration::from_millis(provider.connect_timeout_ms);
        peer.options.connection_timeout = Some(connect_timeout);
        peer.options.total_connection_timeout = Some(connect_timeout);
        peer.options.read_timeout = Some(Duration::from_millis(provider.read_timeout_ms));
        peer.options.write_timeout = Some(Duration::from_millis(provider.write_timeout_ms));
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
        request.insert_header("host", provider.authority())?;
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

    async fn upstream_response_filter(
        &self,
        _session: &mut Session,
        response: &mut ResponseHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()> {
        // This is a copy of the upstream header, before downstream framing is
        // selected. The upstream reader keeps its original framing information.
        let mut nominated = Vec::new();
        for value in response.headers.get_all("connection") {
            let value = value.to_str().map_err(|error| {
                Error::because(
                    ErrorType::InvalidHTTPHeader,
                    "invalid upstream Connection header",
                    error,
                )
                .into_up()
            })?;
            for name in value
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                let name = name.to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "content-length" | "transfer-encoding" | "content-encoding"
                ) {
                    return Err(Error::explain(
                        ErrorType::InvalidHTTPHeader,
                        "upstream Connection header nominates a framing or encoding header",
                    )
                    .into_up());
                }
                nominated.push(name);
            }
        }
        for name in nominated {
            response.remove_header(name.as_str());
        }
        // Transfer-Encoding remains managed by Pingora: removing a transfer
        // coding that the framework has not decoded could change body semantics.
        for name in [
            "connection",
            "keep-alive",
            "proxy-connection",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailer",
            "upgrade",
        ] {
            response.remove_header(name);
        }
        Ok(())
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        _ctx: &mut Self::CTX,
        mut error: Box<Error>,
    ) -> Box<Error> {
        error.set_retry(false);
        error
    }

    fn error_while_proxy(
        &self,
        _peer: &HttpPeer,
        _session: &mut Session,
        mut error: Box<Error>,
        _ctx: &mut Self::CTX,
        _client_reused: bool,
    ) -> Box<Error> {
        // Generation requests must not be replayed, even on a reused connection.
        error.set_retry(false);
        error
    }

    async fn fail_to_proxy(
        &self,
        session: &mut Session,
        error: &Error,
        _ctx: &mut Self::CTX,
    ) -> FailToProxy {
        // A stream that already started can only be terminated. Its HTTP status
        // remains the one the client received; never append an error body to SSE.
        if let Some(response) = session.response_written()
            && (!response.status.is_informational() || response.status.as_u16() == 101)
        {
            return FailToProxy {
                error_code: response.status.as_u16(),
                can_reuse_downstream: false,
            };
        }

        let code = error_status(error);
        if code != 0
            && let Err(write_error) = session.respond_error(code).await
        {
            tracing::warn!(
                error_type = write_error.etype().as_str(),
                "failed to send gateway error"
            );
        }
        FailToProxy {
            error_code: code,
            can_reuse_downstream: false,
        }
    }

    async fn logging(
        &self,
        session: &mut Session,
        error: Option<&pingora::Error>,
        ctx: &mut Self::CTX,
    ) {
        let status = session
            .response_written()
            .map(|header| header.status.as_u16());
        let protocol = ctx.protocol.map(Protocol::as_str).unwrap_or("none");
        let mut attributes = vec![KeyValue::new("protocol", protocol)];
        if let Some(status) = status {
            attributes.push(KeyValue::new("status", status as i64));
        }
        let error_type = error.map(|error| error.etype().as_str());
        if let Some(error_type) = error_type {
            attributes.push(KeyValue::new("error_type", error_type));
        }
        self.requests.add(1, &attributes);
        let failed = error.is_some() || status.is_some_and(|status| status >= 500);
        if failed {
            self.failures.add(1, &attributes);
        }
        let duration = ctx.start.elapsed().as_secs_f64();
        self.duration.record(duration, &attributes);
        if let Some(span) = &ctx.span {
            if let Some(status) = status {
                span.record("http.status_code", status);
            }
            let _entered = span.enter();
            tracing::info!(
                status,
                duration_seconds = duration,
                protocol,
                failed,
                error_type,
                error_source = error.map(|error| error.esource().as_str()),
                "request completed"
            );
        }
    }
}

fn error_status(error: &Error) -> u16 {
    use ErrorType::*;
    if let HTTPStatus(code) = error.etype() {
        return *code;
    }
    match error.esource() {
        ErrorSource::Upstream => match error.etype() {
            ConnectTimedout | TLSHandshakeTimedout | ReadTimedout | WriteTimedout => 504,
            _ => 502,
        },
        ErrorSource::Downstream => match error.etype() {
            WriteError | ReadError | ConnectionClosed => 0,
            ReadTimedout | WriteTimedout => 408,
            _ => 400,
        },
        ErrorSource::Internal | ErrorSource::Unset => 500,
    }
}
