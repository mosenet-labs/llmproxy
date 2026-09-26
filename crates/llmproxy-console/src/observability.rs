use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use llmproxy_store::StoreError;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
    trace::TraceContextExt,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body, LayerFn, Next,
        request::{method, original_uri},
        response::{IntoResponse, Response},
    },
};
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::app::AppState;

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub fn transport_failure(status: Option<u16>) {
    tracing::warn!(
        component = "console",
        event_kind = "transport",
        status,
        "console transport rejected or interrupted"
    );
}

pub struct ConsoleTelemetry {
    requests: Counter<u64>,
    duration: Histogram<f64>,
    operations: Counter<u64>,
}

impl Default for ConsoleTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleTelemetry {
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("llmproxy-console");
        Self {
            requests: meter.u64_counter("llmproxy.console.requests").build(),
            duration: meter
                .f64_histogram("llmproxy.console.request.duration")
                .with_unit("s")
                .build(),
            operations: meter
                .u64_counter("llmproxy.console.provider.operations")
                .build(),
        }
    }

    pub fn provider_operation(
        &self,
        action: &'static str,
        id: Option<i64>,
        name: Option<&str>,
        error: Option<&StoreError>,
    ) {
        let error_kind = match error {
            None => "none",
            Some(StoreError::Validation(_)) => "validation",
            Some(StoreError::Conflict(_)) => "conflict",
            Some(StoreError::Configuration(_)) => "configuration",
            Some(StoreError::NotFound) => "not_found",
            Some(StoreError::Internal) => "storage",
        };
        let outcome = if error.is_some() {
            "failure"
        } else {
            "success"
        };
        self.operations.add(
            1,
            &[
                KeyValue::new("component", "console"),
                KeyValue::new("action", action),
                KeyValue::new("outcome", outcome),
                KeyValue::new("error_kind", error_kind),
            ],
        );
        let (trace_id, span_id) = correlation();
        if error.is_some() {
            Span::current().set_status(opentelemetry::trace::Status::error(error_kind));
            tracing::warn!(
                component = "console",
                event_kind = "provider_operation",
                action,
                provider_id = id,
                provider_name = name,
                outcome,
                error_kind,
                trace_id,
                span_id,
                "provider operation failed"
            );
        } else {
            tracing::info!(
                component = "console",
                event_kind = "provider_operation",
                action,
                provider_id = id,
                provider_name = name,
                outcome,
                trace_id,
                span_id,
                "provider operation completed"
            );
        }
    }
}

// A pathless layer also wraps 404/405 and runs outside Host/Origin/body limits.
pub fn request_layer() -> LayerFn {
    LayerFn::new(None::<&str>, |cx, body, next| {
        Box::pin(observe(cx, body, next))
    })
}

async fn observe(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let telemetry = &app_context::<AppState>(cx).telemetry;
    let route = route_label(original_uri(cx).path());
    let method = match method(cx).as_str() {
        known @ ("GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS") => known,
        _ => "OTHER",
    };
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let span = tracing::info_span!(
        "console.request",
        component = "console",
        otel.kind = "server",
        request_id,
        method,
        route,
        status = tracing::field::Empty
    );
    async {
        let start = Instant::now();
        // Use Topcoat's own conversion so the logged status matches the response.
        let result = next.run(cx, body).await;
        // Page re-runs use the router's native internal rewrite, not an HTTP error.
        if result.as_ref().err().is_some_and(|error| {
            error
                .downcast_ref::<topcoat::router::error::RewriteError>()
                .is_some()
        }) {
            return result;
        }
        let response = result.into_response(cx)?;
        let status = response.status().as_u16();
        let duration = start.elapsed().as_secs_f64();
        Span::current().record("status", status);
        if status >= 500 {
            Span::current().set_status(opentelemetry::trace::Status::error("http_server_error"));
        }
        let attributes = [
            KeyValue::new("component", "console"),
            KeyValue::new("route", route),
            KeyValue::new("method", method.to_owned()),
            KeyValue::new("status", i64::from(status)),
        ];
        telemetry.requests.add(1, &attributes);
        telemetry.duration.record(duration, &attributes);
        let (trace_id, span_id) = correlation();
        tracing::info!(
            component = "console",
            event_kind = "request",
            request_id,
            method,
            route,
            status,
            duration_ms = duration * 1000.0,
            trace_id,
            span_id,
            "console request completed"
        );
        Ok(response)
    }
    .instrument(span)
    .await
}

fn correlation() -> (Option<String>, Option<String>) {
    let context = Span::current().context();
    let span = context.span();
    let context = span.span_context();
    if context.is_valid() {
        (
            Some(context.trace_id().to_string()),
            Some(context.span_id().to_string()),
        )
    } else {
        (None, None)
    }
}

fn route_label(path: &str) -> &'static str {
    match path {
        "/ui" => "/ui",
        "/ui/routes" => "/ui/routes",
        "/ui/providers" => "/ui/providers",
        "/ui/providers/form" => "/ui/providers/form",
        "/ui/providers/save" => "/ui/providers/save",
        "/ui/providers/action" => "/ui/providers/action",
        _ if path.starts_with("/ui/_topcoat/runtime/procedures/") => {
            "/ui/_topcoat/runtime/procedures/:id"
        }
        _ if path.starts_with("/ui/_topcoat/runtime/shards/") => "/ui/_topcoat/runtime/shards/:id",
        _ if path.starts_with("/ui/assets/") || path.starts_with("/_topcoat/") => "assets/runtime",
        _ => "unmatched",
    }
}
