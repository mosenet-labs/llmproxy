use std::{
    io,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use llmproxy_core::protocol::Protocol;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, Meter},
    trace::TraceContextExt,
};
use pingora::{Error, ErrorSource, ErrorType, protocols::Digest, upstreams::peer::Tracer};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::connection::{ConnectionProbe, Connections};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub struct RequestTelemetry {
    start: Instant,
    id: u64,
    span: Option<Span>,
    finished: bool,
    protocol: &'static str,
    route: &'static str,
    upstream: Option<String>,
    address: Option<String>,
    local_address: Option<String>,
    probe: Option<ConnectionProbe>,
    connection_id: Option<u64>,
    reused: Option<bool>,
    connecting: Option<Instant>,
    connected: Option<Instant>,
    dns_seconds: Option<f64>,
    acquire_seconds: Option<f64>,
    tcp_seconds: Option<f64>,
    tls_seconds: Option<f64>,
    header_seconds: Option<f64>,
    tls_version: Option<String>,
    tls_cipher: Option<String>,
    upstream_status: Option<u16>,
    error_stage: Option<&'static str>,
    error_response_write: Option<&'static str>,
}

impl RequestTelemetry {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            id: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
            span: None,
            finished: false,
            protocol: "none",
            route: "unmatched",
            upstream: None,
            address: None,
            local_address: None,
            probe: None,
            connection_id: None,
            reused: None,
            connecting: None,
            connected: None,
            dns_seconds: None,
            acquire_seconds: None,
            tcp_seconds: None,
            tls_seconds: None,
            header_seconds: None,
            tls_version: None,
            tls_cipher: None,
            upstream_status: None,
            error_stage: None,
            error_response_write: None,
        }
    }

    pub fn begin(&mut self, method: &str, path: &str) {
        self.route = match path {
            "/v1/chat/completions" => "/v1/chat/completions",
            "/v1/responses" => "/v1/responses",
            "/v1/messages" => "/v1/messages",
            "/v1/auto" => "/v1/auto",
            _ => "unmatched",
        };
        let path: String = path.chars().take(512).collect();
        self.span = Some(
            tracing::info_span!(parent: None, "llmproxy.request", component = "gateway",
            request_id = self.id, http.method = method, http.route = self.route, http.path = path,
            llm.protocol = tracing::field::Empty, upstream = tracing::field::Empty,
            http.status_code = tracing::field::Empty, otel.kind = "server",
            otel.status_code = tracing::field::Empty),
        );
    }

    pub fn selected(&mut self, protocol: Protocol, upstream: Option<String>) {
        self.protocol = protocol.as_str();
        self.upstream = upstream;
        if let Some(span) = &self.span {
            span.record("llm.protocol", self.protocol);
            if let Some(upstream) = &self.upstream {
                span.record("upstream", upstream);
            }
        }
    }

    pub fn resolved(&mut self, elapsed: Duration, result: &pingora::Result<SocketAddr>) {
        self.dns_seconds = Some(elapsed.as_secs_f64());
        match result {
            Ok(address) => self.address = Some(address.to_string()),
            Err(_) => self.error_stage = Some("dns"),
        }
    }

    pub fn connection_failed(&mut self, error: &Error) {
        self.acquire_seconds = self.connecting.map(|start| start.elapsed().as_secs_f64());
        self.connection_id = self.probe.as_ref().and_then(ConnectionProbe::id);
        self.error_stage = Some(match error.etype() {
            ErrorType::TLSHandshakeFailure
            | ErrorType::TLSHandshakeTimedout
            | ErrorType::InvalidCert => "tls",
            // A total connection timeout does not prove TLS was the failing operation.
            _ => "connect",
        });
    }

    pub fn response_headers(&mut self, status: u16) {
        if status >= 200 || status == 101 {
            self.upstream_status = Some(status);
            self.header_seconds = self.connected.map(|start| start.elapsed().as_secs_f64());
        }
    }

    pub fn error_response_failed(&mut self, error: &Error) {
        self.error_response_write = Some(error.etype().as_str());
    }

    fn failure_stage(&self, error: &Error) -> &'static str {
        if error.esource() == &ErrorSource::Downstream {
            return "downstream";
        }
        if let Some(stage) = self.error_stage {
            return stage;
        }
        match error.etype() {
            ErrorType::WriteError | ErrorType::WriteTimedout => "request_write",
            ErrorType::InvalidHTTPHeader => "response_headers",
            _ if self.connected.is_some() && self.upstream_status.is_none() => "response_headers",
            _ if self.upstream_status.is_some() => "response_body",
            _ => "request",
        }
    }
}

pub struct GatewayTelemetry {
    requests: Counter<u64>,
    failures: Counter<u64>,
    duration: Histogram<f64>,
    phase_duration: Histogram<f64>,
    connections: Connections,
}

impl GatewayTelemetry {
    pub fn new() -> Self {
        Self::with_meter(opentelemetry::global::meter("llmproxy-gateway"))
    }

    fn with_meter(meter: Meter) -> Self {
        Self {
            requests: meter.u64_counter("llmproxy.requests").build(),
            failures: meter.u64_counter("llmproxy.failures").build(),
            duration: meter
                .f64_histogram("llmproxy.duration")
                .with_unit("s")
                .build(),
            phase_duration: meter
                .f64_histogram("llmproxy.phase.duration")
                .with_unit("s")
                .build(),
            connections: Connections::new(meter.u64_counter("llmproxy.connection.events").build()),
        }
    }

    pub fn connecting(&self, request: &mut RequestTelemetry, address: SocketAddr) -> Tracer {
        let probe = self.connections.probe(address);
        let tracer = probe.tracer();
        request.probe = Some(probe);
        request.connecting = Some(Instant::now());
        tracer
    }

    pub fn connected(
        &self,
        request: &mut RequestTelemetry,
        reused: bool,
        handle: u64,
        digest: Option<&Digest>,
    ) {
        request.acquire_seconds = request
            .connecting
            .map(|start| start.elapsed().as_secs_f64());
        request.connected = Some(Instant::now());
        request.reused = Some(reused);
        request.connection_id = if reused {
            self.connections.get(handle)
        } else {
            request.probe.as_ref().and_then(|probe| probe.bind(handle))
        };
        if let Some(digest) = digest {
            if let Some(socket) = &digest.socket_digest {
                request.address = socket
                    .peer_addr()
                    .map(ToString::to_string)
                    .or(request.address.take());
                request.local_address = socket.local_addr().map(ToString::to_string);
            }
            if let Some(tls) = &digest.ssl_digest {
                request.tls_version = Some(tls.version.to_string());
                request.tls_cipher = Some(tls.cipher.to_string());
            }
            if !reused {
                request.tcp_seconds = digest
                    .timing_digest
                    .first()
                    .and_then(Option::as_ref)
                    .and_then(|timing| timing.establishment_duration)
                    .map(|d| d.as_secs_f64());
                if digest.ssl_digest.is_some() {
                    request.tls_seconds = digest
                        .timing_digest
                        .get(1)
                        .and_then(Option::as_ref)
                        .and_then(|timing| timing.establishment_duration)
                        .map(|d| d.as_secs_f64());
                }
            }
        }
    }

    pub fn finish(
        &self,
        request: &mut RequestTelemetry,
        status: Option<u16>,
        error: Option<&Error>,
    ) {
        if request.finished {
            return;
        }
        request.finished = true;
        let duration = request.start.elapsed().as_secs_f64();
        let failed = error.is_some() || status.is_some_and(|s| s >= 500);
        let error_type = error.map(|e| e.etype().as_str());
        let error_stage = error.map(|e| request.failure_stage(e));
        let (io_kind, os_code) = error.map(io_details).unwrap_or_default();
        let mut attributes = vec![
            KeyValue::new("component", "gateway"),
            KeyValue::new("protocol", request.protocol),
            KeyValue::new("route", request.route),
        ];
        if let Some(status) = status {
            attributes.push(KeyValue::new("status", i64::from(status)));
        }
        if let Some(kind) = error_type {
            attributes.push(KeyValue::new("error_type", kind));
        }
        if let Some(stage) = error_stage {
            attributes.push(KeyValue::new("error_stage", stage));
        }

        // Enter only for this synchronous emission, never across an await.
        let span = request.span.take();
        let _entered = span.as_ref().map(Span::enter);
        self.requests.add(1, &attributes);
        if failed {
            self.failures.add(1, &attributes);
        }
        self.duration.record(duration, &attributes);
        for (phase, elapsed) in [
            ("dns", request.dns_seconds),
            ("connection_acquire", request.acquire_seconds),
            ("tcp", request.tcp_seconds),
            ("tls", request.tls_seconds),
            ("upstream_headers", request.header_seconds),
        ] {
            if let Some(elapsed) = elapsed {
                let mut labels = vec![
                    KeyValue::new("component", "gateway"),
                    KeyValue::new("protocol", request.protocol),
                    KeyValue::new("phase", phase),
                ];
                labels.push(KeyValue::new(
                    "outcome",
                    if (phase == "dns" && error_stage == Some("dns"))
                        || (phase == "connection_acquire" && request.reused.is_none())
                    {
                        "error"
                    } else {
                        "success"
                    },
                ));
                self.phase_duration.record(elapsed, &labels);
            }
        }
        if let Some(span) = &span {
            if let Some(status) = status {
                span.record("http.status_code", status);
            }
            if failed {
                span.record("otel.status_code", "ERROR");
            }
        }
        let context = span
            .as_ref()
            .map(OpenTelemetrySpanExt::context)
            .unwrap_or_default();
        let otel_span = context.span();
        let correlation = otel_span.span_context();
        tracing::info!(
            component = "gateway",
            event_kind = "request",
            request_id = request.id,
            trace_id = correlation
                .is_valid()
                .then(|| correlation.trace_id().to_string()),
            span_id = correlation
                .is_valid()
                .then(|| correlation.span_id().to_string()),
            protocol = request.protocol,
            route = request.route,
            upstream = request.upstream,
            upstream_address = request.address,
            local_address = request.local_address,
            status,
            upstream_status = request.upstream_status,
            duration_seconds = duration,
            connection_id = request.connection_id,
            connection_reused = request.reused,
            tcp_connected = request.connection_id.is_some(),
            dns_seconds = request.dns_seconds,
            connection_acquire_seconds = request.acquire_seconds,
            tcp_seconds = request.tcp_seconds,
            tls_seconds = request.tls_seconds,
            upstream_header_seconds = request.header_seconds,
            tls_version = request.tls_version,
            tls_cipher = request.tls_cipher,
            failed,
            error_type,
            error_source = error.map(|e| e.esource().as_str()),
            error_stage,
            error_reason = error.map(error_reason),
            error_io_kind = io_kind,
            error_os_code = os_code,
            error_response_write = request.error_response_write,
            "request completed"
        );
    }
}

// Never format arbitrary error context/cause: HTTP errors can embed headers/body.
fn io_details(error: &Error) -> (Option<String>, Option<i32>) {
    let mut current: &(dyn std::error::Error + 'static) = error;
    for _ in 0..8 {
        if let Some(io) = current.downcast_ref::<io::Error>() {
            return (Some(format!("{:?}", io.kind())), io.raw_os_error());
        }
        let next = if let Some(pingora) = current.downcast_ref::<Error>() {
            pingora
                .cause
                .as_deref()
                .map(|cause| cause as &(dyn std::error::Error + 'static))
        } else {
            current.source()
        };
        match next {
            Some(next) => current = next,
            None => break,
        }
    }
    (None, None)
}

fn error_reason(error: &Error) -> &'static str {
    use ErrorType::*;
    match error.etype() {
        ConnectTimedout => "connection deadline exceeded",
        ConnectRefused => "connection refused",
        ConnectNoRoute => "network route unavailable",
        TLSHandshakeTimedout => "TLS handshake deadline exceeded",
        TLSHandshakeFailure | InvalidCert => "TLS handshake or certificate verification failed",
        ReadTimedout => "read deadline exceeded",
        WriteTimedout => "write deadline exceeded",
        ConnectionClosed => "connection closed",
        InvalidHTTPHeader | H1Error | H2Error => "invalid HTTP message",
        ConnectError => "address resolution or connection failed",
        ReadError => "stream read failed",
        WriteError => "stream write failed",
        _ => "request processing failed",
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
