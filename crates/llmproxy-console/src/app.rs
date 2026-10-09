use llmproxy_store::ProviderStore;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body, Layer, LayerFuture, Next, Path, error::forbidden, layer, request::headers,
        response::Response,
    },
};

pub(crate) mod chat_service;
pub(crate) mod chat_sessions;
pub(crate) mod holiday_notice;
pub(crate) mod model_catalog;
pub(crate) mod ui;

pub fn route_builder() -> topcoat::router::RouterBuilder {
    topcoat::router::module_router!()
}

pub struct AppState {
    pub websocket: bool,
    pub subscriptions: crate::SubscriptionPresence,
    pub store: ProviderStore,
    pub health: std::sync::Arc<crate::model_health::ModelHealthService>,
    pub csrf: String,
    pub history_auth: String,
    pub port: u16,
    pub telemetry: std::sync::Arc<crate::observability::ConsoleTelemetry>,
    pub chat_sessions: std::sync::Arc<chat_sessions::ChatSessions>,
    pub gateway_origin: String,
}

#[layer("/")]
pub async fn protect(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    validate_origin(cx)?;
    let mut response = next.run(cx, body).await?;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse()?);
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse()?);
    response
        .headers_mut()
        .insert("x-frame-options", "DENY".parse()?);
    response
        .headers_mut()
        .insert("referrer-policy", "same-origin".parse()?);
    Ok(response)
}

fn validate_origin(cx: &Cx) -> Result<()> {
    let state = app_context::<AppState>(cx);
    let request_headers = headers(cx);
    let authority = request_headers
        .get("host")
        .and_then(|host| host.to_str().ok());
    let localhost = format!("localhost:{}", state.port);
    let loopback = format!("127.0.0.1:{}", state.port);
    if !matches!(authority, Some(host) if host == localhost || host == loopback) {
        return Err(forbidden().into());
    }
    if let Some(origin) = request_headers.get("origin") {
        let allowed = origin.to_str().ok().is_some_and(|origin| {
            origin == format!("http://{localhost}") || origin == format!("http://{loopback}")
        });
        if !allowed {
            return Err(forbidden().into());
        }
    }
    Ok(())
}

// RuntimeLayer accepts a handshake before dispatching normal page guards.
async fn protect_connection(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let socket = headers(cx).contains_key("sec-websocket-protocol")
        || topcoat::runtime::connected_untracked(cx);
    if socket {
        validate_origin(cx)?;
        let path = topcoat::router::request::uri(cx).path();
        if !app_context::<AppState>(cx).websocket || !(path == "/ui" || path.starts_with("/ui/")) {
            return Err(forbidden().into());
        }
    }
    next.run(cx, body).await
}

pub(crate) fn request_connection(cx: &Cx) -> bool {
    app_context::<AppState>(cx).websocket && topcoat::runtime::connected(cx)
}

pub(crate) fn check_csrf(cx: &Cx, supplied: &str) -> Result<()> {
    let expected = app_context::<AppState>(cx).csrf.as_bytes();
    let supplied = supplied.as_bytes();
    let difference = expected
        .iter()
        .zip(supplied)
        .fold(0, |acc, (a, b)| acc | (a ^ b));
    if supplied.len() != expected.len() || difference != 0 {
        return Err(forbidden().into());
    }
    Ok(())
}

pub(crate) struct ConnectionGuard;

impl Layer for ConnectionGuard {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(protect_connection(cx, body, next))
    }
}

/// Cookie 固定在页面与 runtime 握手的请求上下文中；切组使用完整导航。
pub(crate) fn group_store(cx: &Cx, scope: &str) -> ProviderStore {
    let prefix = format!("llmproxy_{scope}_group=");
    let group = headers(cx)
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .find_map(|cookie| cookie.trim().strip_prefix(&prefix))
                .or_else(|| {
                    cookies
                        .split(';')
                        .find_map(|cookie| cookie.trim().strip_prefix("llmproxy_group="))
                })
        })
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .unwrap_or(1);
    app_context::<AppState>(cx).store.for_group(group)
}
