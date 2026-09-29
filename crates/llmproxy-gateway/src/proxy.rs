use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::Bytes;
use llmproxy_core::{
    protocol::{MessagesAuth, Protocol},
    routing::{Route, match_route},
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use pingora::{
    Error, ErrorSource, ErrorType, Result,
    protocols::Digest,
    proxy::{FailToProxy, ProxyHttp, Session},
    upstreams::peer::HttpPeer,
};
use pingora_http::{RequestHeader, ResponseHeader};

use crate::{
    observability::{GatewayTelemetry, RequestTelemetry},
    snapshot::{ProviderSnapshots, ResolvedProvider},
    transform::{BodyTransform, ModelRead, RequestBody, response_kind},
};

const MODEL_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub struct Gateway {
    providers: ProviderSnapshots,
    telemetry: GatewayTelemetry,
    console: llmproxy_console::Console,
}

pub struct RequestContext {
    telemetry: RequestTelemetry,
    protocol: Option<Protocol>,
    provider: Option<Arc<ResolvedProvider>>,
    console: bool,
    gemini_model_id: Option<String>,
    gemini_stream: bool,
    request_body: RequestBody,
    response_body: BodyTransform,
}

impl Gateway {
    pub fn new(providers: ProviderSnapshots, console: llmproxy_console::Console) -> Self {
        Self {
            providers,
            telemetry: GatewayTelemetry::new(),
            console,
        }
    }

    fn resolve_model_route(
        &self,
        protocol: Protocol,
        alias: &str,
    ) -> std::result::Result<(Arc<ResolvedProvider>, String), u16> {
        let model = self.providers.select(protocol, alias).ok_or(404u16)?;
        let provider = model.provider.as_ref().ok_or(503u16)?.clone();
        Ok((provider, model.upstream_model_id.clone()))
    }
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = RequestContext;

    fn new_ctx(&self) -> Self::CTX {
        RequestContext {
            telemetry: RequestTelemetry::new(),
            protocol: None,
            provider: None,
            console: false,
            gemini_model_id: None,
            gemini_stream: false,
            request_body: RequestBody::new(),
            response_body: BodyTransform::default(),
        }
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        // WEB控制台
        if crate::console::matches(session.req_header().uri.path()) {
            ctx.console = true;
            crate::console::serve(&self.console, session).await?;
            return Ok(true);
        }
        let request = session.req_header();
        let method = request.method.as_str();
        let path = request.uri.path();
        let route = match_route(method, path);
        ctx.telemetry.begin(method, path);

        match route {
            Route::Gemini { alias, stream } => {
                let protocol = Protocol::Gemini;
                ctx.protocol = Some(protocol);
                let (provider, upstream_model_id) = match self.resolve_model_route(protocol, &alias)
                {
                    Ok(route) => route,
                    Err(status) => {
                        ctx.telemetry.selected(protocol, None);
                        session.respond_error(status).await?;
                        return Ok(true);
                    }
                };
                ctx.telemetry.selected(protocol, Some(provider.authority()));
                ctx.provider = Some(provider);
                ctx.gemini_model_id = Some(upstream_model_id);
                ctx.gemini_stream = stream;
                Ok(false)
            }
            Route::Proxy(protocol) => {
                ctx.protocol = Some(protocol);
                let content_encoding = session.get_header_bytes("content-encoding");
                if !content_encoding.is_empty()
                    && !content_encoding.eq_ignore_ascii_case(b"identity")
                {
                    session.set_keepalive(None);
                    session.respond_error(415).await?;
                    return Ok(true);
                }
                // 持续读取从session中读取request body数据直到读取model为止
                let alias = match ctx.request_body.read_model(session).await? {
                    ModelRead::Found(alias) => alias,
                    ModelRead::Rejected(status) => {
                        session.set_keepalive(None);
                        session.respond_error(status).await?;
                        return Ok(true);
                    }
                };
                let (provider, upstream_model_id) = match self.resolve_model_route(protocol, &alias)
                {
                    Ok(route) => route,
                    Err(status) => {
                        ctx.telemetry.selected(protocol, None);
                        session.set_keepalive(None);
                        session.respond_error(status).await?;
                        return Ok(true);
                    }
                };
                ctx.request_body
                    .select_model(&upstream_model_id)
                    .map_err(|_| {
                        Error::explain(ErrorType::InternalError, "cannot encode upstream model")
                    })?;
                // Pin one immutable provider for the full request, including SSE.
                ctx.telemetry.selected(protocol, Some(provider.authority()));
                ctx.provider = Some(provider);
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
        let provider = ctx
            .provider
            .as_ref()
            .expect("request_filter selected provider");
        // Resolve before constructing HttpPeer, which would panic on a DNS error.
        let connect_timeout = Duration::from_millis(provider.connect_timeout_ms);
        let dns_start = Instant::now();
        let address = match tokio::time::timeout(
            connect_timeout,
            tokio::net::lookup_host((provider.host.as_str(), provider.port)),
        )
        .await
        {
            Ok(Ok(mut addresses)) => addresses.next().ok_or_else(|| {
                Error::explain(
                    ErrorType::ConnectError,
                    "provider DNS lookup returned no addresses",
                )
                .into_up()
            }),
            Ok(Err(error)) => {
                Err(
                    Error::because(ErrorType::ConnectError, "provider DNS lookup failed", error)
                        .into_up(),
                )
            }
            Err(_) => Err(Error::explain(
                ErrorType::ConnectTimedout,
                "provider DNS lookup timed out",
            )
            .into_up()),
        };
        ctx.telemetry.resolved(dns_start.elapsed(), &address);
        let address = address?;
        let mut peer = HttpPeer::new(address, provider.tls, provider.host.clone());
        peer.options.tracer = Some(self.telemetry.connecting(&mut ctx.telemetry, address));
        peer.options.connection_timeout = Some(connect_timeout);
        peer.options.total_connection_timeout = Some(connect_timeout);
        peer.options.read_timeout = Some(Duration::from_millis(provider.read_timeout_ms));
        peer.options.write_timeout = Some(Duration::from_millis(provider.write_timeout_ms));
        Ok(Box::new(peer))
    }

    async fn connected_to_upstream(
        &self,
        _session: &mut Session,
        reused: bool,
        _peer: &HttpPeer,
        #[cfg(unix)] fd: std::os::unix::io::RawFd,
        #[cfg(windows)] sock: std::os::windows::io::RawSocket,
        digest: Option<&Digest>,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        #[cfg(unix)]
        let handle = fd as u64;
        #[cfg(windows)]
        let handle = sock as u64;
        self.telemetry
            .connected(&mut ctx.telemetry, reused, handle, digest);
        Ok(())
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        let protocol = ctx.protocol.expect("request_filter set protocol");
        let provider = ctx
            .provider
            .as_ref()
            .expect("request_filter selected provider");
        let base_path = if protocol == Protocol::Gemini {
            let model = ctx
                .gemini_model_id
                .as_deref()
                .expect("Gemini route selected model");
            let model = model.strip_prefix("models/").unwrap_or(model);
            let encoded = utf8_percent_encode(model, MODEL_SEGMENT);
            let method = if ctx.gemini_stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            };
            format!(
                "{}/{}:{method}",
                provider.upstream_path.trim_end_matches('/'),
                encoded
            )
        } else {
            provider.upstream_path.clone()
        };
        let query = request.uri.query().unwrap_or_default();
        let path = if ctx.gemini_stream && !query.split('&').any(|part| part == "alt=sse") {
            format!(
                "{base_path}?{query}{}alt=sse",
                if query.is_empty() { "" } else { "&" }
            )
        } else if query.is_empty() {
            base_path
        } else {
            format!("{base_path}?{query}")
        };
        request.set_uri(
            path.parse().map_err(|_| {
                Error::explain(ErrorType::InvalidHTTPHeader, "invalid upstream path")
            })?,
        );
        request.remove_header("authorization");
        request.remove_header("x-api-key");
        request.remove_header("x-goog-api-key");
        request.insert_header("host", provider.authority())?;
        if let Some(length) = request.headers.get("content-length") {
            let length = length
                .to_str()
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| {
                    Error::explain(ErrorType::InvalidHTTPHeader, "invalid content length")
                })?;
            let adjusted = length
                .checked_add_signed(ctx.request_body.body_delta())
                .ok_or_else(|| {
                    Error::explain(
                        ErrorType::InvalidHTTPHeader,
                        "invalid rewritten content length",
                    )
                })?;
            request.insert_header("content-length", adjusted.to_string())?;
        }
        request.remove_header("content-md5");
        request.remove_header("digest");
        match protocol {
            Protocol::AnthropicMessages => {
                if provider.messages_auth == MessagesAuth::Bearer {
                    request
                        .insert_header("authorization", format!("Bearer {}", provider.secret))?;
                } else {
                    request.insert_header("x-api-key", provider.secret.as_str())?;
                }
                if let Some(version) = &provider.anthropic_version {
                    request.insert_header("anthropic-version", version.as_str())?;
                }
            }
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                request.insert_header("authorization", format!("Bearer {}", provider.secret))?;
            }
            Protocol::Gemini => {
                request.insert_header("x-goog-api-key", provider.secret.as_str())?;
            }
        }
        Ok(())
    }

    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        ctx.request_body.push(body, end_of_stream)
    }

    async fn upstream_response_filter(
        &self,
        _session: &mut Session,
        response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        ctx.telemetry.response_headers(response.status.as_u16());
        // 在正文回调开始前，根据上游响应头选择分帧方式。
        ctx.response_body.replace_kind(response_kind(
            response
                .headers
                .get("content-type")
                .map(|value| value.as_bytes()),
            response
                .headers
                .get("content-encoding")
                .map(|value| value.as_bytes()),
        ));
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

    fn upstream_response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>> {
        ctx.response_body.push(body, end_of_stream);
        Ok(None)
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut Self::CTX,
        mut error: Box<Error>,
    ) -> Box<Error> {
        ctx.telemetry.connection_failed(&error);
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
        ctx: &mut Self::CTX,
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
            ctx.telemetry.error_response_failed(&write_error);
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
        if ctx.console {
            if error.is_some() {
                llmproxy_console::observability::transport_failure(
                    session
                        .response_written()
                        .map(|header| header.status.as_u16()),
                );
            }
            return;
        }
        let status = session
            .response_written()
            .map(|header| header.status.as_u16());
        self.telemetry.finish(&mut ctx.telemetry, status, error);
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
