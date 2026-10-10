use llmproxy_store::ProviderStore;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body, Layer, LayerFuture, Next, Path, error::forbidden, layer, request::headers,
        response::Response,
    },
};

pub(crate) mod auth;
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
    pub auth: auth::AuthService,
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
    auth::guard(cx, body, next).await
}

fn validate_origin(cx: &Cx) -> Result<()> {
    let state = app_context::<AppState>(cx);
    let request_headers = headers(cx);
    let authority = request_headers
        .get("host")
        .and_then(|host| host.to_str().ok());
    if !authority.is_some_and(|host| state.auth.allows_host(host, state.port)) {
        return Err(forbidden().into());
    }
    if let Some(origin) = request_headers.get("origin") {
        let allowed = origin
            .to_str()
            .ok()
            .is_some_and(|origin| state.auth.allows_origin(origin, state.port));
        if !allowed {
            return Err(forbidden().into());
        }
    }
    Ok(())
}

// RuntimeLayer accepts a handshake before dispatching normal page guards.
async fn protect_connection(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let response = topcoat::router::response::response_headers(cx);
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "same-origin"),
    ] {
        response.append(
            name.parse::<topcoat::router::header::HeaderName>()?,
            value.parse()?,
        );
    }
    validate_origin(cx)?;
    let socket = headers(cx).contains_key("sec-websocket-protocol")
        || topcoat::runtime::connected_untracked(cx);
    if socket {
        let path = topcoat::router::request::uri(cx).path();
        if !app_context::<AppState>(cx).websocket || !(path == "/ui" || path.starts_with("/ui/")) {
            return Err(forbidden().into());
        }
    }
    auth::guard(cx, body, next).await
}

pub(crate) fn request_connection(cx: &Cx) -> bool {
    app_context::<AppState>(cx).websocket && topcoat::runtime::connected(cx)
}

pub(crate) fn check_csrf(cx: &Cx, supplied: &str) -> Result<()> {
    let token = auth::csrf_token(cx);
    let expected = token.as_bytes();
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

/// 当前会话授权的资源目录；普通用户使用个人作用域。
pub(crate) fn store(cx: &Cx) -> ProviderStore {
    topcoat::context::request_context::<auth::ResourceStore>(cx)
        .store
        .clone()
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
        .unwrap_or_else(|| store(cx).group_id());
    let store = store(cx);
    let resources = topcoat::context::request_context::<auth::ResourceStore>(cx);
    let group = resources
        .groups
        .iter()
        .find(|row| row.id == group)
        .map(|row| row.id)
        .unwrap_or_else(|| store.group_id());
    store.for_group(group)
}

/// Pin navigation to the current resource space, including links opened in a new tab.
pub(crate) fn scoped_href(cx: &Cx, href: impl AsRef<str>) -> String {
    let href = href.as_ref();
    let Some(resources) = topcoat::context::try_request_context::<auth::ResourceStore>(cx) else {
        return href.to_owned();
    };
    if !href.starts_with("/ui") || href.starts_with("/ui/assets/") || href == "/ui/login" {
        return href.to_owned();
    }
    let mut url = url::Url::parse("http://console.invalid")
        .unwrap()
        .join(href)
        .unwrap();
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "space")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    pairs.push(("space".into(), resources.space.id.to_string()));
    url.query_pairs_mut().clear().extend_pairs(pairs);
    url[url::Position::BeforePath..].to_owned()
}
