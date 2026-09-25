use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use llmproxy_core::{
    protocol::Protocol,
    routing::{Route, match_route},
};
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
};

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
}

impl Gateway {
    pub fn new(providers: ProviderSnapshots, console: llmproxy_console::Console) -> Self {
        Self {
            providers,
            telemetry: GatewayTelemetry::new(),
            console,
        }
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
        }
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
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
            Route::Proxy(protocol) => {
                ctx.protocol = Some(protocol);
                // Pin one immutable provider for the full request, including SSE.
                // Later refreshes affect only requests entering after selection.
                ctx.provider = self.providers.select(protocol);
                ctx.telemetry
                    .selected(protocol, ctx.provider.as_ref().map(|p| p.authority()));
                if ctx.provider.is_none() {
                    session.respond_error(503).await?;
                    return Ok(true);
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
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        ctx.telemetry.response_headers(response.status.as_u16());
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
