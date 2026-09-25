use super::*;
use opentelemetry::{
    metrics::MeterProvider,
    trace::{Status, TracerProvider},
};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::{
    logs::{InMemoryLogExporter, SdkLoggerProvider},
    metrics::{
        InMemoryMetricExporter, SdkMeterProvider,
        data::{AggregatedMetrics, MetricData},
    },
    trace::{InMemorySpanExporter, SdkTracerProvider},
};
use pingora::protocols::{TimingDigest, tls::digest::SslDigest};
use std::{sync::Arc, time::SystemTime};
use tracing_subscriber::layer::SubscriberExt;

// These tests exercise the same tracing callsites with different subscribers.
// Isolate subscriber setup/teardown and callsite registration between them.
static TELEMETRY_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn completion_correlates_signals_once_and_does_not_retain_request_span() {
    let _test = TELEMETRY_TEST.lock().unwrap();
    let spans = InMemorySpanExporter::default();
    let traces = SdkTracerProvider::builder()
        .with_simple_exporter(spans.clone())
        .build();
    let logs = InMemoryLogExporter::default();
    let logger = SdkLoggerProvider::builder()
        .with_simple_exporter(logs.clone())
        .build();
    let metrics = InMemoryMetricExporter::default();
    let meters = SdkMeterProvider::builder()
        .with_periodic_exporter(metrics.clone())
        .build();
    let reporter = GatewayTelemetry::with_meter(meters.meter("test"));
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(traces.tracer("test")))
        .with(OpenTelemetryTracingBridge::new(&logger));
    tracing::subscriber::with_default(subscriber, || {
        let mut request = RequestTelemetry::new();
        request.begin("POST", "/v1/chat/completions");
        request.selected(Protocol::OpenAiChat, Some("provider.invalid".into()));
        let tracer = reporter.connecting(&mut request, "127.0.0.1:443".parse().unwrap());
        tracer.0.on_connected();
        let error = Error::because(
            ErrorType::TLSHandshakeFailure,
            "secret-in-context",
            io::Error::new(io::ErrorKind::InvalidData, "secret-in-cause"),
        )
        .into_up();
        request.connection_failed(&error);
        reporter.finish(&mut request, Some(502), Some(&error));
        reporter.finish(&mut request, Some(502), Some(&error));
        // Neither CTX nor the pooled stream's tracer may delay span completion.
        assert_eq!(spans.get_finished_spans().unwrap().len(), 1);
        tracer.0.on_disconnected();
    });
    meters.force_flush().unwrap();
    let spans = spans.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 1);
    assert!(matches!(spans[0].status, Status::Error { .. }));
    let logs = logs.get_emitted_logs().unwrap();
    let completion: Vec<_> = logs
        .iter()
        .filter(|log| {
            log.record.attributes_iter().any(|(key, value)| {
                key.as_str() == "event_kind" && format!("{value:?}").contains("request")
            })
        })
        .collect();
    assert_eq!(completion.len(), 1, "exported logs: {logs:?}");
    let context = completion[0].record.trace_context().unwrap();
    assert_eq!(context.trace_id, spans[0].span_context.trace_id());
    assert_eq!(context.span_id, spans[0].span_context.span_id());
    let exported = format!("{logs:?}{spans:?}");
    assert!(!exported.contains("secret-in-"));
    assert!(exported.contains("InvalidData"));
    let batches = metrics.get_finished_metrics().unwrap();
    for name in ["llmproxy.requests", "llmproxy.failures"] {
        let metric = batches
            .iter()
            .flat_map(|b| b.scope_metrics())
            .flat_map(|s| s.metrics())
            .find(|m| m.name() == name)
            .unwrap();
        let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected counter")
        };
        assert_eq!(sum.data_points().map(|p| p.value()).sum::<u64>(), 1);
        for point in sum.data_points() {
            assert!(point.attributes().all(|a| matches!(
                a.key.as_str(),
                "component" | "protocol" | "route" | "status" | "error_type" | "error_stage"
            )));
            assert!(
                point
                    .attributes()
                    .any(|a| { a.key.as_str() == "component" && a.value.as_str() == "gateway" })
            );
        }
    }
}

#[test]
fn reused_digest_does_not_double_count_handshakes_and_release_clears_identity() {
    let _test = TELEMETRY_TEST.lock().unwrap();
    let metrics = InMemoryMetricExporter::default();
    let meters = SdkMeterProvider::builder()
        .with_periodic_exporter(metrics.clone())
        .build();
    let reporter = GatewayTelemetry::with_meter(meters.meter("test"));
    let mut first = RequestTelemetry::new();
    let tracer = reporter.connecting(&mut first, "127.0.0.1:443".parse().unwrap());
    tracer.0.on_connected();
    let digest = Digest {
        ssl_digest: Some(Arc::new(SslDigest {
            version: "TLSv1.3".into(),
            cipher: "TLS_AES_256_GCM_SHA384".into(),
            organization: None,
            serial_number: None,
            cert_digest: vec![],
            extension: Default::default(),
        })),
        timing_digest: [2, 3]
            .into_iter()
            .map(|ms| {
                Some(TimingDigest {
                    established_ts: SystemTime::now(),
                    establishment_duration: Some(Duration::from_millis(ms)),
                    offload_wait_duration: None,
                })
            })
            .collect(),
        ..Default::default()
    };
    reporter.connected(&mut first, false, 42, Some(&digest));
    reporter.finish(&mut first, Some(200), None);
    let mut reused = RequestTelemetry::new();
    let unused_tracer = reporter.connecting(&mut reused, "127.0.0.1:443".parse().unwrap());
    reporter.connected(&mut reused, true, 42, Some(&digest));
    assert_eq!(first.connection_id, reused.connection_id);
    assert!(first.connection_id.is_some());
    assert_eq!(reused.tls_version.as_deref(), Some("TLSv1.3"));
    assert_eq!(reused.tcp_seconds, None);
    assert_eq!(reused.tls_seconds, None);
    reporter.finish(&mut reused, Some(200), None);
    drop(unused_tracer);
    tracer.0.on_disconnected();
    assert_eq!(reporter.connections.get(42), None);
    // Reused OS handles receive a new connection identity.
    let mut replacement = RequestTelemetry::new();
    let next = reporter.connecting(&mut replacement, "127.0.0.1:443".parse().unwrap());
    next.0.on_connected();
    reporter.connected(&mut replacement, false, 42, None);
    assert_ne!(replacement.connection_id, first.connection_id);
    assert_eq!(replacement.tcp_seconds, None);
    assert_eq!(replacement.tls_seconds, None);
    next.0.on_disconnected();
    meters.force_flush().unwrap();
    let batches = metrics.get_finished_metrics().unwrap();
    let metric = batches
        .iter()
        .flat_map(|b| b.scope_metrics())
        .flat_map(|s| s.metrics())
        .find(|m| m.name() == "llmproxy.phase.duration")
        .unwrap();
    let AggregatedMetrics::F64(MetricData::Histogram(hist)) = metric.data() else {
        panic!("expected histogram")
    };
    for phase in ["tcp", "tls"] {
        let points: Vec<_> = hist
            .data_points()
            .filter(|p| {
                p.attributes()
                    .any(|a| a.key.as_str() == "phase" && a.value.as_str() == phase)
            })
            .collect();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].count(), 1);
    }
}
