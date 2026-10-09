mod buffered;
mod models;
mod request;
mod route;
use route::SelectedRoute;
#[cfg(test)]
mod request_tests;
mod streaming;
mod thinking;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::Bytes;
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
    transform::{BodyTransform, MessagePhase, ModelRead, RequestBody, response_kind},
};

pub struct Gateway {
    history_store: llmproxy_store::ProviderStore,
    providers: ProviderSnapshots,
    telemetry: GatewayTelemetry,
    console: llmproxy_console::Console,
    tool_states: Arc<crate::tool_state::Cache>,
    subscriptions: crate::subscriptions::Hub,
}

pub struct RequestContext {
    identity: Option<llmproxy_store::CallIdentity>,
    group_id: i64,
    history: Option<crate::history::Tracker>,
    thinking: llmproxy_core::thinking::Choice,
    thinking_error: Option<&'static str>,
    telemetry: RequestTelemetry,
    protocol: Option<Protocol>,
    // 未选路时只保留入口协议供错误响应使用；选路结果作为整体移交子请求。
    route: Option<SelectedRoute>,
    console: bool,
    stream_error: Option<Bytes>,
    request_body: RequestBody,
    response_body: BodyTransform,
}

impl Gateway {
    pub fn new(
        providers: ProviderSnapshots,
        console: llmproxy_console::Console,
        store: llmproxy_store::ProviderStore,
    ) -> Self {
        Self {
            subscriptions: crate::subscriptions::Hub::new(
                store.clone(),
                console.subscription_presence(),
            ),
            history_store: store.clone(),
            providers,
            telemetry: GatewayTelemetry::new(),
            console,
            tool_states: Arc::new(crate::tool_state::Cache::database(store)),
        }
    }

    fn resolve_model_route(
        &self,
        protocol: Protocol,
        alias: &str,
        ctx: &RequestContext,
    ) -> std::result::Result<
        (
            Arc<ResolvedProvider>,
            String,
            llmproxy_core::thinking::Config,
        ),
        u16,
    > {
        let (model, route_id) =
            self.providers
                .select_for(ctx.group_id, ctx.identity.as_ref(), protocol, alias)?;
        tracing::info!(
            group_id = ctx.group_id,
            virtual_key_id = ctx.identity.as_ref().map(|identity| identity.key_id),
            route_id,
            "model route selected"
        );
        if model.blocked {
            return Err(503);
        }
        let provider = model.provider.as_ref().ok_or(503u16)?.clone();
        Ok((
            provider,
            model.upstream_model_id.clone(),
            model.thinking.clone(),
        ))
    }

    /// 统一准备路由、转换模式和工具作用域，Pingora 阶段只负责读取及选择。
    async fn prepare_route(
        &self,
        _session: &Session,
        ctx: &mut RequestContext,
        route: SelectedRoute,
    ) -> Result<()> {
        if let Err(message) = route.thinking.check(ctx.thinking) {
            ctx.thinking_error = Some(message);
            return Err(Error::explain(ErrorType::HTTPStatus(422), message));
        }
        if route.provider.protocol == Protocol::Gemini && route.is_cross_protocol() {
            let caller_scope = ctx.identity.as_ref().map_or_else(
                || format!("console-group:{}", ctx.group_id),
                |identity| format!("virtual-key:{}:{}", identity.group_id, identity.key_id),
            );
            let context = crate::tool_state::Context::new(
                self.tool_states.clone(),
                &route.provider,
                &route.upstream_model,
                route.protocol,
                &route.client_model,
                [caller_scope.as_bytes(), b"", b""],
            );
            ctx.request_body.set_tool_state(context.clone());
            ctx.response_body.set_tool_state(context);
        }
        if route.is_cross_protocol() || ctx.thinking != llmproxy_core::thinking::Choice::Default {
            ctx.request_body.set_cross_protocol(
                route.protocol,
                route.provider.protocol,
                &route.upstream_model,
            );
        }
        if let Some(history) = &ctx.history {
            history
                .start(
                    &route.client_model,
                    route.protocol,
                    llmproxy_store::chat_history::ActualCall {
                        provider_id: route.provider.id,
                        provider_name: route.provider.name.clone(),
                        upstream_model: route.upstream_model.clone(),
                        protocol: route.provider.protocol,
                        reasoning: None,
                        reported_model: None,
                    },
                )
                .await?;
            ctx.response_body.set_history(history.sink.clone());
        }
        ctx.route = Some(route);
        Ok(())
    }
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = RequestContext;

    fn new_ctx(&self) -> Self::CTX {
        RequestContext {
            identity: None,
            group_id: 1,
            history: None,
            thinking: Default::default(),
            thinking_error: None,
            telemetry: RequestTelemetry::new(),
            protocol: None,
            route: None,
            console: false,
            stream_error: None,
            request_body: RequestBody::new(),
            response_body: BodyTransform::default(),
        }
    }

    fn allow_spawning_subrequest(&self, session: &Session, _ctx: &Self::CTX) -> bool {
        session.subrequest_ctx.is_none()
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        if buffered::resume(session, ctx)? {
            return Ok(false);
        }
        if crate::subscriptions::matches(session.req_header().uri.path()) {
            // 订阅入口提前返回，先识别遥测路由，使成功轮询／心跳按 DEBUG 记录。
            let request = session.req_header();
            ctx.telemetry
                .begin(request.method.as_str(), request.uri.path());
            self.subscriptions.serve(session).await?;
            return Ok(true);
        }
        // WEB控制台
        if crate::console::matches(session.req_header().uri.path()) {
            ctx.console = true;
            crate::console::serve(&self.console, session).await?;
            return Ok(true);
        }
        let request = session.req_header();
        ctx.telemetry
            .begin(request.method.as_str(), request.uri.path());
        ctx.protocol = match match_route(request.method.as_str(), request.uri.path()) {
            Route::Proxy(protocol) => Some(protocol),
            Route::Gemini { .. } => Some(Protocol::Gemini),
            _ => None,
        };
        let credential = authentication_key(session);
        let internal = credential
            .as_ref()
            .is_ok_and(|secret| self.console.accepts_history_auth(secret.as_bytes()));
        if internal {
            ctx.group_id = session
                .req_header()
                .headers
                .get("x-llmproxy-group")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let enabled = tokio::time::timeout(
                Duration::from_secs(5),
                self.history_store.for_group(ctx.group_id).group_enabled(),
            )
            .await
            .map_err(|_| Error::explain(ErrorType::HTTPStatus(503), "authentication unavailable"))?
            .map_err(|_| {
                Error::explain(ErrorType::HTTPStatus(503), "authentication unavailable")
            })?;
            if !enabled {
                return Err(Error::explain(ErrorType::HTTPStatus(403), "group disabled"));
            }
        } else if models::matches(session.req_header().uri.path())
            || !matches!(
                match_route(
                    session.req_header().method.as_str(),
                    session.req_header().uri.path()
                ),
                Route::NotFound
            )
        {
            let secret = credential?;
            let identity = tokio::time::timeout(
                Duration::from_secs(5),
                self.history_store.authenticate_virtual_key(&secret),
            )
            .await
            .map_err(|_| Error::explain(ErrorType::HTTPStatus(503), "authentication unavailable"))?
            .map_err(|_| Error::explain(ErrorType::HTTPStatus(503), "authentication unavailable"))?
            .ok_or_else(|| Error::explain(ErrorType::HTTPStatus(401), "invalid API key"))?;
            ctx.group_id = identity.group_id;
            tracing::info!(
                group_id = identity.group_id,
                virtual_key_id = identity.key_id,
                "model request authenticated"
            );
            ctx.identity = Some(identity);
        }
        if models::matches(session.req_header().uri.path()) {
            let Some(identity) = &ctx.identity else {
                return Err(Error::explain(
                    ErrorType::HTTPStatus(403),
                    "model catalog requires virtual key",
                ));
            };
            models::serve(&self.providers, identity, session).await?;
            return Ok(true);
        }
        if internal
            && let Some(key) = session
                .req_header()
                .headers
                .get(llmproxy_store::chat_history::REQUEST_HEADER)
                .and_then(|v| v.to_str().ok())
        {
            ctx.history = Some(crate::history::Tracker::new(
                key.into(),
                self.history_store.for_group(ctx.group_id),
            ));
        }
        let request = session.req_header();
        let method = request.method.as_str();
        let path = request.uri.path();
        let route = match_route(method, path);
        match thinking::choice(request) {
            Ok(choice) => ctx.thinking = choice,
            Err(message) => {
                session.set_keepalive(None);
                let protocol = match &route {
                    Route::Proxy(protocol) => Some(*protocol),
                    Route::Gemini { .. } => Some(Protocol::Gemini),
                    _ => None,
                };
                ctx.protocol = protocol;
                crate::transform::respond_error_message(session, protocol, 400, message).await?;
                return Ok(true);
            }
        }

        match route {
            Route::Gemini { alias, stream } => {
                let protocol = Protocol::Gemini;
                ctx.protocol = Some(protocol);
                ctx.request_body.set_protocol(protocol);
                let (provider, upstream_model_id, thinking) =
                    match self.resolve_model_route(protocol, &alias, ctx) {
                        Ok(route) => route,
                        Err(status) => {
                            ctx.telemetry.selected(protocol, None);
                            crate::transform::respond_error(session, ctx.protocol, status).await?;
                            return Ok(true);
                        }
                    };
                ctx.telemetry.selected(protocol, Some(provider.authority()));
                let content_encoding = session.get_header_bytes("content-encoding");
                if (provider.protocol != protocol
                    || ctx.thinking != llmproxy_core::thinking::Choice::Default)
                    && !content_encoding.is_empty()
                    && !content_encoding.eq_ignore_ascii_case(b"identity")
                {
                    crate::transform::respond_error(session, ctx.protocol, 415).await?;
                    return Ok(true);
                }
                self.prepare_route(
                    session,
                    ctx,
                    SelectedRoute::new(
                        protocol,
                        alias,
                        provider,
                        upstream_model_id,
                        stream,
                        thinking,
                    ),
                )
                .await?;
                buffered::forward(self, session, ctx).await
            }
            Route::Proxy(protocol) => {
                ctx.protocol = Some(protocol);
                ctx.request_body.set_protocol(protocol);
                let content_encoding = session.get_header_bytes("content-encoding");
                if !content_encoding.is_empty()
                    && !content_encoding.eq_ignore_ascii_case(b"identity")
                {
                    session.set_keepalive(None);
                    crate::transform::respond_error(session, ctx.protocol, 415).await?;
                    return Ok(true);
                }
                // 持续读取从session中读取request body数据直到读取model为止
                let alias = match ctx.request_body.read_model(session).await? {
                    ModelRead::Found(alias) => alias,
                    ModelRead::Rejected(status) => {
                        session.set_keepalive(None);
                        crate::transform::respond_error(session, ctx.protocol, status).await?;
                        return Ok(true);
                    }
                };
                let (provider, upstream_model_id, thinking) =
                    match self.resolve_model_route(protocol, &alias, ctx) {
                        Ok(route) => route,
                        Err(status) => {
                            ctx.telemetry.selected(protocol, None);
                            session.set_keepalive(None);
                            crate::transform::respond_error(session, ctx.protocol, status).await?;
                            return Ok(true);
                        }
                    };
                ctx.request_body
                    .select_model(&upstream_model_id)
                    .map_err(|_| {
                        Error::explain(ErrorType::InternalError, "cannot encode upstream model")
                    })?;
                ctx.telemetry.selected(protocol, Some(provider.authority()));
                self.prepare_route(
                    session,
                    ctx,
                    SelectedRoute::new(
                        protocol,
                        alias,
                        provider,
                        upstream_model_id,
                        false,
                        thinking,
                    ),
                )
                .await?;
                buffered::forward(self, session, ctx).await
            }
            Route::Auto => {
                crate::transform::respond_error(session, ctx.protocol, 501).await?;
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
                crate::transform::respond_error(session, ctx.protocol, 404).await?;
                Ok(true)
            }
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let provider = &ctx
            .route
            .as_ref()
            .expect("request_filter selected route")
            .provider;
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
        request::filter(request, ctx)
    }

    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        ctx.telemetry
            .instrument(ctx.request_body.push(body, end_of_stream))
            .await
    }

    async fn upstream_response_filter(
        &self,
        _session: &mut Session,
        response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        ctx.telemetry.response_headers(response.status.as_u16());
        // 在正文回调开始前，根据上游响应头选择分帧方式。
        let kind = response_kind(
            response
                .headers
                .get("content-type")
                .map(|value| value.as_bytes()),
            response
                .headers
                .get("content-encoding")
                .map(|value| value.as_bytes()),
        );
        let route = ctx.route.as_ref().expect("request_filter selected route");
        let client_protocol = route.protocol;
        let provider_protocol = route.provider.protocol;
        if client_protocol != provider_protocol {
            response.remove_header("content-length");
            response.remove_header("content-md5");
            response.remove_header("digest");
            response.insert_header(
                "content-type",
                if route.stream && response.status.is_success() {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )?;
            if response.status.is_success() {
                let expected = if route.stream {
                    matches!(kind, crate::transform::BodyKind::Sse)
                } else {
                    matches!(kind, crate::transform::BodyKind::Json)
                };
                if !expected {
                    return Err(Error::explain(
                        ErrorType::HTTPStatus(502),
                        "cross-protocol response format mismatch",
                    )
                    .into_up());
                }
                if route.stream {
                    // 父请求逐帧转换并异步提交签名，子请求只交付未经压缩的来源 SSE。
                    ctx.response_body
                        .replace_kind(crate::transform::BodyKind::Passthrough);
                    response.remove_header("etag");
                    response.insert_header("cache-control", "no-store")?;
                } else {
                    let target = route.response_target();
                    ctx.response_body.set_cross_response(
                        provider_protocol,
                        client_protocol,
                        target.model,
                        target.id,
                        target.created,
                    );
                }
            } else {
                ctx.response_body
                    .set_cross_error(client_protocol, response.status.as_u16());
                response.remove_header("content-encoding");
                response.remove_header("etag");
            }
        }
        if !(route.stream && client_protocol != provider_protocol && response.status.is_success()) {
            ctx.response_body.replace_kind(kind);
        }
        ctx.telemetry.in_scope(|| {
            ctx.response_body
                .set_codec(provider_protocol, MessagePhase::Response)
        });
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
        ctx.telemetry
            .in_scope(|| ctx.response_body.push(body, end_of_stream))?;
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
        // 已发头的跨协议流以客户端错误事件结束，HTTP 状态保留已发送的值。
        if let Some(response) = session.response_written()
            && (!response.status.is_informational() || response.status.as_u16() == 101)
        {
            let status = response.status.as_u16();
            if let Some(body) = ctx.stream_error.take() {
                let timeout = std::time::Duration::from_millis(
                    ctx.route
                        .as_ref()
                        .expect("stream route selected")
                        .provider
                        .write_timeout_ms,
                );
                // 慢客户端导致原发送超时后，错误事件同样受写超时约束。
                let written =
                    tokio::time::timeout(timeout, session.write_response_body(Some(body), true))
                        .await
                        .unwrap_or_else(|_| {
                            Err(Error::explain(
                                ErrorType::WriteTimedout,
                                "client error write timed out",
                            )
                            .into_down())
                        });
                if let Err(write_error) = written {
                    ctx.telemetry.error_response_failed(&write_error);
                }
            }
            return FailToProxy {
                error_code: status,
                can_reuse_downstream: false,
            };
        }

        let code = error_status(error);
        if let Some(message) = ctx.thinking_error {
            // 拒绝后连接不复用，响应头必须与 FailToProxy 的关闭决定一致。
            session.set_keepalive(None);
            if let Err(write_error) =
                crate::transform::respond_error_message(session, ctx.protocol, code, message).await
            {
                ctx.telemetry.error_response_failed(&write_error);
            }
            return FailToProxy {
                error_code: code,
                can_reuse_downstream: false,
            };
        }
        if code != 0
            && let Err(write_error) =
                crate::transform::respond_error(session, ctx.protocol, code).await
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
        if buffered::complete(self, session, ctx) {
            return;
        }
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
        if let Some(history) = &ctx.history
            && history.finish().await.is_err()
        {
            tracing::error!(
                component = "gateway",
                event_kind = "history_save",
                "来源用量保存失败"
            );
        }
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

fn authentication_key(session: &Session) -> Result<String> {
    let mut supplied: Option<&str> = None;
    for name in ["authorization", "x-api-key", "x-goog-api-key"] {
        for value in session.req_header().headers.get_all(name).iter() {
            let raw = value
                .to_str()
                .map_err(|_| Error::explain(ErrorType::HTTPStatus(401), "invalid API key"))?;
            let value = if name == "authorization" {
                raw.split_once(' ')
                    .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
                    .map(|(_, key)| key)
                    .ok_or_else(|| {
                        Error::explain(ErrorType::HTTPStatus(401), "invalid authorization")
                    })?
            } else {
                raw
            };
            if value.is_empty() || supplied.is_some_and(|previous| previous != value) {
                return Err(Error::explain(
                    ErrorType::HTTPStatus(401),
                    "ambiguous API key",
                ));
            }
            supplied = Some(value);
        }
    }
    supplied
        .map(str::to_owned)
        .ok_or_else(|| Error::explain(ErrorType::HTTPStatus(401), "API key required"))
}
