use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use llmproxy_telemetry::init;
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    common::v1::any_value::Value,
    resource::v1::Resource,
};
use prost::Message;

// Run real global subscribers/exporters in separate processes, never mutate
// the test runner's environment or global OTEL providers.
#[test]
fn export_probe() {
    if std::env::var_os("LLMPROXY_OTLP_PROBE").is_none() {
        return;
    }
    let guard = init().unwrap();
    let counter = opentelemetry::global::meter("test")
        .u64_counter("test.requests")
        .build();
    let span = tracing::info_span!("test.request");
    span.in_scope(|| {
        tracing::info!(event_kind = "request", "test request completed");
        counter.add(1, &[]);
    });
    drop(span);
    drop(guard); // All three batch exporters must flush on normal exit.
}

#[test]
fn exports_all_signals_with_unified_identity_and_log_trace_correlation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/api/test", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let collector = {
        let requests = requests.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("collector accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                let mut length = None;
                let mut stream_name = None;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = Some(value.trim().parse::<usize>().unwrap());
                        } else if key.eq_ignore_ascii_case("stream-name") {
                            stream_name = Some(value.trim().to_owned());
                        }
                    }
                }
                let mut body = vec![0; length.expect("OTLP content length")];
                reader.read_exact(&mut body).unwrap();
                requests.lock().unwrap().push((path, stream_name, body));
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-protobuf\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
        })
    };
    let output = Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("RUST_LOG", "info")
        .env("LLMPROXY_OTLP_PROBE", "1")
        .env("LLMPROXY_GATEWAY_SERVICE_NAME", "test-gateway")
        .env("LLMPROXY_CONSOLE_SERVICE_NAME", "test-console")
        .env("OTEL_SERVICE_NAME", "llmproxy")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)
        .env("OTEL_EXPORTER_OTLP_HEADERS", "stream-name=llmproxy")
        .env("OTEL_EXPORTER_OTLP_TIMEOUT", "2000")
        .args(["--exact", "export_probe", "--nocapture"])
        .output()
        .unwrap();
    stop.store(true, Ordering::Relaxed);
    collector.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = requests.lock().unwrap();
    let mut trace_id = None;
    let mut log_trace_id = None;
    let mut has_metric = false;
    for (path, stream_name, body) in requests.iter() {
        assert_eq!(stream_name.as_deref(), Some("llmproxy"));
        match path.as_str() {
            "/api/test/v1/traces" => {
                let data = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
                for resource in data.resource_spans {
                    assert_eq!(service_name(&resource.resource), "llmproxy");
                    for scope in resource.scope_spans {
                        for span in scope.spans {
                            if span.name == "test.request" {
                                trace_id = Some(span.trace_id);
                            }
                        }
                    }
                }
            }
            "/api/test/v1/logs" => {
                let data = ExportLogsServiceRequest::decode(body.as_slice()).unwrap();
                for resource in data.resource_logs {
                    assert_eq!(service_name(&resource.resource), "llmproxy");
                    for scope in resource.scope_logs {
                        for log in scope.log_records {
                            if !log.trace_id.is_empty() {
                                log_trace_id = Some(log.trace_id);
                            }
                        }
                    }
                }
            }
            "/api/test/v1/metrics" => {
                let data = ExportMetricsServiceRequest::decode(body.as_slice()).unwrap();
                for resource in data.resource_metrics {
                    assert_eq!(service_name(&resource.resource), "llmproxy");
                    has_metric |= resource.scope_metrics.iter().any(|scope| {
                        scope
                            .metrics
                            .iter()
                            .any(|metric| metric.name == "test.requests")
                    });
                }
            }
            other => panic!("unexpected OTLP path {other}"),
        }
    }
    assert!(trace_id.is_some(), "missing trace");
    assert_eq!(trace_id, log_trace_id, "log/trace correlation");
    assert!(has_metric, "missing metric");
}

fn service_name(resource: &Option<Resource>) -> &str {
    resource
        .as_ref()
        .unwrap()
        .attributes
        .iter()
        .find_map(|attribute| {
            if attribute.key != "service.name" {
                return None;
            }
            match attribute.value.as_ref()?.value.as_ref()? {
                Value::StringValue(value) => Some(value.as_str()),
                _ => None,
            }
        })
        .expect("service.name resource attribute")
}
