//! Shared process telemetry setup. Business events stay in each application.

use std::{env, error::Error};

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::{
    Resource, logs::SdkLoggerProvider, metrics::SdkMeterProvider, trace::SdkTracerProvider,
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

fn service_name(lookup: impl Fn(&str) -> Option<String>) -> String {
    lookup("OTEL_SERVICE_NAME")
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "llmproxy".to_owned())
}

pub struct TelemetryGuard {
    tracer: Option<SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
    meter: Option<SdkMeterProvider>,
}

pub fn init() -> Result<TelemetryGuard, Box<dyn Error + Send + Sync>> {
    let service_name = service_name(|key| env::var(key).ok());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,llmproxy=info,llmproxy_gateway=info,llmproxy_console=info,llmproxy_telemetry=info")
    });
    let json = tracing_subscriber::fmt::layer().json();

    if env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok_and(|value| !value.trim().is_empty()) {
        let resource = Resource::builder()
            .with_service_name(service_name.clone())
            .build();

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
            .try_init()?;

        tracing::info!(
            component = "app",
            event_kind = "runtime",
            service_name,
            otlp = true,
            "telemetry initialized"
        );
        Ok(TelemetryGuard {
            tracer: Some(tracer),
            logger: Some(logger),
            meter: Some(meter),
        })
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(json)
            .try_init()?;
        tracing::info!(
            component = "app",
            event_kind = "runtime",
            service_name,
            otlp = false,
            "telemetry initialized"
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_service_name_uses_standard_variable_and_default() {
        assert_eq!(service_name(|_| None), "llmproxy");
        assert_eq!(service_name(|_| Some("  ".to_owned())), "llmproxy");
        assert_eq!(
            service_name(|key| {
                assert_eq!(key, "OTEL_SERVICE_NAME");
                Some("custom-app".to_owned())
            }),
            "custom-app"
        );
    }
}
