use std::{env, error::Error};

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::{
    Resource, logs::SdkLoggerProvider, metrics::SdkMeterProvider, trace::SdkTracerProvider,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

mod connection;
mod request;

pub use request::{GatewayTelemetry, RequestTelemetry};

pub fn listening(listen: &str) {
    tracing::info!(event_kind = "runtime", listen, "gateway listening");
}

pub fn snapshot_refresh(available: bool) {
    if available {
        tracing::info!(
            event_kind = "runtime",
            "provider snapshot refresh recovered"
        );
    } else {
        // Database errors can include URLs, SQL values, or credentials.
        tracing::warn!(
            event_kind = "runtime",
            "provider snapshot refresh failed; retaining the last snapshot"
        );
    }
}

pub struct TelemetryGuard {
    tracer: Option<SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
    meter: Option<SdkMeterProvider>,
}

pub fn init() -> Result<TelemetryGuard, Box<dyn Error + Send + Sync>> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("llmproxy_gateway=info,pingora=warn"));
    let json = tracing_subscriber::fmt::layer().json();

    if env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some() {
        let service_name =
            env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "llmproxy-gateway".to_owned());
        let resource = Resource::builder().with_service_name(service_name).build();

        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build()?;
        let tracer = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(span_exporter)
            .build();

        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .build()?;
        let logger = SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(log_exporter)
            .build();

        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .build()?;
        let meter = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_periodic_exporter(metric_exporter)
            .build();
        opentelemetry::global::set_meter_provider(meter.clone());

        let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer.tracer("llmproxy"));
        let log_layer = OpenTelemetryTracingBridge::new(&logger);
        tracing_subscriber::registry()
            .with(filter)
            .with(json)
            .with(trace_layer)
            .with(log_layer)
            .init();

        Ok(TelemetryGuard {
            tracer: Some(tracer),
            logger: Some(logger),
            meter: Some(meter),
        })
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(json)
            .init();
        Ok(TelemetryGuard {
            tracer: None,
            logger: None,
            meter: None,
        })
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = &self.tracer {
            let _ = provider.shutdown();
        }
        if let Some(provider) = &self.logger {
            let _ = provider.shutdown();
        }
        if let Some(provider) = &self.meter {
            let _ = provider.shutdown();
        }
    }
}
