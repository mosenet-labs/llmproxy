use llmproxy_store::{
    StoreError,
    auth::{AuthSession, UserRole, normalize_email},
};
use std::{env, io, time::Duration};
use topcoat::{
    Result,
    context::{Cx, app_context, try_request_context},
    router::{
        Body, Next,
        header::{HeaderMap, SET_COOKIE},
        request::{client_ip, headers, method, uri},
        response::{Response, response_headers},
    },
};

use super::AppState;
pub(crate) mod mail;
mod procedures;
mod rate;
pub(crate) use procedures::*;

pub(crate) const SESSION_COOKIE: &str = "llmproxy_session";
pub(crate) type Outcome = std::result::Result<String, String>;

pub(crate) struct AuthService {
    rates: rate::RateLimits,
    public_origin: Option<String>,
}

impl AuthService {
    pub(crate) fn from_env() -> io::Result<Self> {
        let public_origin = env::var("LLMPROXY_PUBLIC_ORIGIN")
            .ok()
            .map(|origin| {
                let url = url::Url::parse(&origin)
                    .map_err(|_| io::Error::other("LLMPROXY_PUBLIC_ORIGIN 须为有效的控制台地址"))?;
                let local = url.host_str().is_some_and(|h| {
                    h == "localhost"
                        || h.parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                if !(url.scheme() == "https" || (url.scheme() == "http" && local))
                    || url.path() != "/"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(io::Error::other(
                        "LLMPROXY_PUBLIC_ORIGIN 须为 HTTPS origin，本地回环地址可使用 HTTP",
                    ));
                }
                Ok(url.origin().ascii_serialization())
            })
            .transpose()?;
        Ok(Self {
            rates: Default::default(),
            public_origin,
        })
    }

    pub(crate) fn secure(&self) -> bool {
        self.public_origin
            .as_ref()
            .is_some_and(|s| s.starts_with("https://"))
    }

    pub(crate) fn allows_origin(&self, origin: &str, port: u16) -> bool {
        match &self.public_origin {
            Some(public) => public == origin,
            None => {
                origin == format!("http://localhost:{port}")
                    || origin == format!("http://127.0.0.1:{port}")
            }
        }
    }

    pub(crate) fn allows_host(&self, host: &str, port: u16) -> bool {
        match &self.public_origin {
            Some(public) => public
                .split_once("://")
                .is_some_and(|(_, authority)| authority == host),
            None => host == format!("localhost:{port}") || host == format!("127.0.0.1:{port}"),
        }
    }

    fn attempt(
        &self,
        cx: &Cx,
        purpose: &str,
        email: &str,
        mail: bool,
    ) -> std::result::Result<(), String> {
        let email = normalize_email(email).map_err(message)?;
        let window = Duration::from_secs(if mail { 3600 } else { 900 });
        let ip = client_ip(cx)
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "unknown".into());
        self.rates.take(
            format!("{purpose}:ip:{ip}"),
            if mail { 20 } else { 60 },
            window,
        )?;
        self.rates.take(
            format!("{purpose}:email:{email}"),
            if mail { 5 } else { 10 },
            window,
        )
    }
}

pub(crate) fn session_secret(headers: &HeaderMap) -> Option<&str> {
    let mut cookies = headers
        .get_all("cookie")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|p| p.trim().split_once('='))
        .filter(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value);
    let value = cookies.next()?;
    if cookies.next().is_some() {
        return None;
    }
    Some(value)
}

pub(crate) fn session(cx: &Cx) -> Result<&AuthSession> {
    try_request_context::<AuthSession>(cx)
        .ok_or_else(|| topcoat::router::error::unauthorized().into())
}

pub(crate) fn csrf_token(cx: &Cx) -> String {
    try_request_context::<AuthSession>(cx)
        .map(|s| s.csrf.clone())
        .unwrap_or_else(|| app_context::<AppState>(cx).csrf.clone())
}

pub(crate) fn message(error: StoreError) -> String {
    match error {
        StoreError::Validation(text) | StoreError::Conflict(text) => text,
        _ => "账户操作失败，请稍后重试或联系管理员".into(),
    }
}

fn public_path(path: &str) -> bool {
    path == "/ui/login"
        || path.starts_with("/ui/assets/")
        || path.starts_with("/ui/_topcoat/fonts/")
        || path.starts_with("/_topcoat/fonts/")
        || matches!(
            path,
            "/ui/_topcoat/runtime/procedures/auth-login"
                | "/ui/_topcoat/runtime/procedures/auth-bootstrap-admin"
                | "/ui/_topcoat/runtime/procedures/auth-send-code"
                | "/ui/_topcoat/runtime/procedures/auth-register"
                | "/ui/_topcoat/runtime/procedures/auth-reset-password"
        )
}

pub(crate) struct ResourceStore {
    pub store: llmproxy_store::ProviderStore,
    pub groups: Vec<llmproxy_store::GroupView>,
}

fn member_path(path: &str) -> bool {
    matches!(
        path,
        "/ui" | "/ui/" | "/ui/account" | "/ui/chat" | "/ui/keys"
    ) || [
        "/ui/providers",
        "/ui/models",
        "/ui/routes",
        "/ui/subscriptions",
        "/ui/groups",
    ]
    .iter()
    .any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }) || matches!(
        path,
        "/ui/_topcoat/runtime/procedures/claim-subscription"
            | "/ui/_topcoat/runtime/shards/key-workspace"
            | "/ui/_topcoat/runtime/procedures/save-group"
            | "/ui/_topcoat/runtime/procedures/create-key"
            | "/ui/_topcoat/runtime/procedures/change-key"
            | "/ui/_topcoat/runtime/shards/model-workspace"
            | "/ui/_topcoat/runtime/shards/model-candidates"
            | "/ui/_topcoat/runtime/shards/provider-list"
            | "/ui/_topcoat/runtime/procedures/save-route"
            | "/ui/_topcoat/runtime/procedures/delete-route"
            | "/ui/_topcoat/runtime/procedures/import-subscription-models"
            | "/ui/_topcoat/runtime/procedures/remove-group-resource"
            | "/ui/_topcoat/runtime/procedures/save-group-resources"
            | "/ui/_topcoat/runtime/procedures/load-model-candidates"
            | "/ui/_topcoat/runtime/procedures/save-models"
            | "/ui/_topcoat/runtime/procedures/save-model"
            | "/ui/_topcoat/runtime/procedures/delete-model"
            | "/ui/_topcoat/runtime/procedures/probe-saved-model"
            | "/ui/_topcoat/runtime/procedures/remove-draft-model"
            | "/ui/_topcoat/runtime/procedures/add-draft-model"
            | "/ui/_topcoat/runtime/shards/model-health"
            | "/ui/_topcoat/runtime/procedures/save-health-check"
            | "/ui/_topcoat/runtime/shards/model-draft"
            | "/ui/_topcoat/runtime/procedures/add-price-rule"
            | "/ui/_topcoat/runtime/procedures/remove-price-rule"
            | "/ui/_topcoat/runtime/procedures/add-price-window"
            | "/ui/_topcoat/runtime/procedures/remove-price-window"
            | "/ui/_topcoat/runtime/procedures/deepseek-peak-windows"
            | "/ui/_topcoat/runtime/procedures/preview-price-band"
            | "/ui/_topcoat/runtime/procedures/save-price-plan"
            | "/ui/_topcoat/runtime/shards/price-rule-rows"
            | "/ui/_topcoat/runtime/shards/price-matrix-rows"
            | "/ui/_topcoat/runtime/shards/price-peak-windows"
            | "/ui/_topcoat/runtime/procedures/preview-models"
            | "/ui/_topcoat/runtime/procedures/save-provider"
            | "/ui/_topcoat/runtime/procedures/provider-action"
            | "/ui/_topcoat/runtime/shards/provider-health"
            | "/ui/_topcoat/runtime/procedures/new-chat"
            | "/ui/_topcoat/runtime/procedures/switch-chat"
            | "/ui/_topcoat/runtime/procedures/default-chat-protocol"
            | "/ui/_topcoat/runtime/procedures/begin-chat"
            | "/ui/_topcoat/runtime/shards/chat-stop-button"
            | "/ui/_topcoat/runtime/procedures/stop-chat"
            | "/ui/_topcoat/runtime/procedures/send-chat"
            | "/ui/_topcoat/runtime/shards/chat-protocol-picker"
            | "/ui/_topcoat/runtime/shards/chat-session-list"
            | "/ui/_topcoat/runtime/shards/chat-session-activity"
            | "/ui/_topcoat/runtime/procedures/open-chat"
            | "/ui/_topcoat/runtime/procedures/change-chat-history"
            | "/ui/_topcoat/runtime/shards/chat-history"
            | "/ui/_topcoat/runtime/procedures/set-chat-thinking"
            | "/ui/_topcoat/runtime/shards/chat-thinking-picker"
            | "/ui/_topcoat/runtime/shards/chat-usage"
            | "/ui/_topcoat/runtime/procedures/retry-chat-save"
            | "/ui/_topcoat/runtime/shards/chat-model-picker"
            | "/ui/_topcoat/runtime/shards/chat-health-notice"
            | "/ui/_topcoat/runtime/procedures/reprobe-chat-model"
            | "/ui/_topcoat/runtime/shards/chat-health-updates"
            | "/ui/_topcoat/runtime/procedures/auth-logout"
            | "/ui/_topcoat/runtime/procedures/auth-save-profile"
            | "/ui/_topcoat/runtime/procedures/auth-change-password"
    )
}

pub(crate) async fn guard(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let path = uri(cx).path();
    let state = app_context::<AppState>(cx);
    let identity = if let Some(secret) = session_secret(headers(cx)) {
        state.store.authenticate_session(secret).await?
    } else {
        None
    };
    if let Some(identity) = identity {
        if !public_path(path) && !member_path(path) && identity.user.role != UserRole::Admin {
            return Err(topcoat::router::error::forbidden().into());
        }
        let resources = if identity.user.role == UserRole::Admin {
            state.store.clone()
        } else {
            Box::pin(state.store.for_user(identity.user.id)).await?
        };
        let groups = Box::pin(resources.list_groups()).await?;
        let child = cx.with(identity);
        let child = child.with(ResourceStore {
            store: resources,
            groups,
        });
        return next.run(&child, body).await;
    }
    if public_path(path)
        && !topcoat::runtime::connected_untracked(cx)
        && !headers(cx).contains_key("sec-websocket-protocol")
    {
        return next.run(cx, body).await;
    }
    if method(cx) == "GET" && !headers(cx).contains_key("sec-websocket-protocol") {
        return Err(topcoat::router::error::see_other("/ui/login").into());
    }
    Err(topcoat::router::error::unauthorized().into())
}

fn set_cookie(cx: &Cx, secret: &str, age: u64) -> Result<()> {
    let secure = if app_context::<AppState>(cx).auth.secure() {
        "; Secure"
    } else {
        ""
    };
    response_headers(cx).append(
        SET_COOKIE,
        format!(
            "{SESSION_COOKIE}={secret}; Path=/ui; HttpOnly; SameSite=Strict; Max-Age={age}{secure}"
        )
        .parse()?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_unique_and_public_origin_is_exact() {
        let mut headers = HeaderMap::new();
        headers.append("cookie", "other=x; llmproxy_session=first".parse().unwrap());
        assert_eq!(session_secret(&headers), Some("first"));
        headers.append("cookie", "llmproxy_session=second".parse().unwrap());
        assert_eq!(session_secret(&headers), None);
        let service = AuthService {
            rates: Default::default(),
            public_origin: Some("https://console.example.test".into()),
        };
        assert!(service.secure());
        assert!(service.allows_host("console.example.test", 3200));
        assert!(!service.allows_host("console.example.test.attacker.test", 3200));
        assert!(service.allows_origin("https://console.example.test", 3200));
        assert!(!service.allows_origin("http://console.example.test", 3200));
        assert!(!service.allows_origin("https://console.example.test:8443", 3200));
    }
}
