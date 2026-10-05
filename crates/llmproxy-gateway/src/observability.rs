mod connection;
mod request;
mod usage;

pub use request::{GatewayTelemetry, RequestTelemetry};
pub use usage::response_usage;

pub fn listening(listen: &str) {
    tracing::info!(
        component = "gateway",
        event_kind = "runtime",
        listen,
        "gateway listening"
    );
}

pub fn snapshot_refresh(available: bool) {
    if available {
        tracing::info!(
            component = "gateway",
            event_kind = "runtime",
            "provider snapshot refresh recovered"
        );
    } else {
        // Database errors can include URLs, SQL values, or credentials.
        tracing::warn!(
            component = "gateway",
            event_kind = "runtime",
            "provider snapshot refresh failed; retaining the last snapshot"
        );
    }
}
