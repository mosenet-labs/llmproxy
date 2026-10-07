use llmproxy_store::ProviderStore;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{Body, Next, error::forbidden, layer, request::headers, response::Response},
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
    pub store: ProviderStore,
    pub prober: llmproxy_probe::ModelProber,
    pub csrf: String,
    pub history_auth: String,
    pub port: u16,
    pub telemetry: crate::observability::ConsoleTelemetry,
    pub chat_sessions: std::sync::Arc<chat_sessions::ChatSessions>,
    pub chat_client: reqwest::Client,
    pub gateway_origin: String,
}

#[layer("/")]
pub async fn protect(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
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
