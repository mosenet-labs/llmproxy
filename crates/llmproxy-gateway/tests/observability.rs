mod support;

use serde_json::Value;
use std::{
    io::Write,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::*;

fn completions(gateway: &Gateway) -> Vec<Value> {
    gateway
        .logs()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| {
            entry["fields"]["event_kind"] == "request"
                && !entry["span"]["http.path"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("/__llmproxy_test_ready/")
        })
        .map(|entry| entry["fields"].clone())
        .collect()
}

fn wait_completions(gateway: &Gateway, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let records = completions(gateway);
        if records.len() >= count {
            assert_eq!(records.len(), count);
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "missing records: {}",
            gateway.logs()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn http_and_local_rejections_have_one_sanitized_completion_each() {
    let (upstream, _) = Mock::http(|_, stream| respond(stream, 429, "", b"private-provider-body"));
    let gateway = Gateway::start([Provider::http(upstream.address); 3]);
    for path in PATHS {
        let response = gateway.request(
            "POST",
            &format!("{path}?token=private-query"),
            "Authorization: Bearer private-client-key\r\n",
            b"private-prompt",
        );
        assert_eq!(response.status, 429);
        assert_eq!(response.body(), b"private-provider-body");
    }
    for (method, path, status) in [
        ("GET", "/v1/messages", 405),
        ("POST", "/v1/auto", 501),
        ("GET", "/missing", 404),
    ] {
        let response = gateway.request(method, path, "", b"");
        assert_eq!(response.status, status);
        response.body();
    }
    let records = wait_completions(&gateway, 6);
    for record in &records[..3] {
        assert_eq!(record["status"], 429);
        assert_eq!(record["upstream_status"], 429);
        assert_eq!(record["failed"], false);
        assert_eq!(record["connection_reused"], false);
        assert!(record["dns_seconds"].is_number());
        assert!(record["tcp_seconds"].is_number());
        assert!(record["tls_seconds"].is_null());
        assert!(record["error_type"].is_null());
    }
    assert_eq!(records[5]["route"], "unmatched");
    for record in &records[3..] {
        assert!(record["connection_id"].is_null());
    }
    let logs = gateway.logs();
    for secret in [
        "private-query",
        "private-client-key",
        "private-prompt",
        "private-provider-body",
    ]
    .into_iter()
    .chain(SECRETS)
    {
        assert!(!logs.contains(secret), "sensitive value in logs");
    }
}

#[test]
fn actual_connection_reuse_preserves_identity_without_handshake_samples() {
    let upstream = Mock::raw(|mut stream| {
        for _ in 0..2 {
            Request::read(&mut stream).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap();
            stream.flush().unwrap();
        }
    });
    let gateway = Gateway::start([Provider::http(upstream.address); 3]);
    for count in 1..=2 {
        let response = gateway.request("POST", PATHS[0], "", b"{}");
        assert_eq!(response.status, 200);
        assert_eq!(response.body(), b"{}");
        wait_completions(&gateway, count);
    }
    let records = wait_completions(&gateway, 2);
    assert_eq!(upstream.count(), 1);
    assert_eq!(records[0]["connection_reused"], false);
    assert_eq!(records[1]["connection_reused"], true);
    assert_eq!(records[0]["connection_id"], records[1]["connection_id"]);
    assert!(records[0]["connection_id"].is_number());
    assert!(records[0]["tcp_seconds"].is_number());
    assert!(records[1]["tcp_seconds"].is_null());
}

#[test]
fn dns_and_connection_failures_include_stage_without_fake_tls_timing() {
    let mut reservation = Some(bind_listener("127.0.0.1:0"));
    let address = reservation.as_ref().unwrap().local_addr().unwrap();
    for dns in [false, true] {
        let mut provider = Provider::http(address);
        if dns {
            provider.host = Some("llmproxy-observation-test.invalid");
        }
        let gateway = Gateway::start([provider; 3]);
        drop(reservation.take());
        let response = gateway.request("POST", PATHS[0], "", b"{}");
        assert!(matches!(response.status, 502 | 504));
        response.body();
        let records = wait_completions(&gateway, 1);
        assert_eq!(
            records[0]["error_stage"],
            if dns { "dns" } else { "connect" }
        );
        assert_eq!(records[0]["failed"], true);
        assert!(records[0]["error_type"].is_string());
        assert!(records[0]["dns_seconds"].is_number());
        assert!(records[0]["tcp_seconds"].is_null());
        assert!(records[0]["tls_seconds"].is_null());
    }
}

#[test]
fn sse_completion_waits_for_stream_end_and_retains_written_status_on_error() {
    let (release, hold) = mpsc::channel();
    let hold = std::sync::Mutex::new(hold);
    let (upstream, _) = Mock::http(move |_, stream| {
        sse_headers(stream);
        chunk(stream, b"data: private-token\n\n").unwrap();
        hold.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        // Deliberately omit the terminating chunk.
    });
    let gateway = Gateway::start([Provider::http(upstream.address); 3]);
    let mut response = gateway.request("POST", PATHS[0], "", b"{}");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.bytes(b"data: private-token\n\n".len()),
        b"data: private-token\n\n"
    );
    assert!(completions(&gateway).is_empty());
    release.send(()).unwrap();
    assert!(response.next_chunk().is_err());
    let records = wait_completions(&gateway, 1);
    assert_eq!(records[0]["status"], 200);
    assert_eq!(records[0]["upstream_status"], 200);
    assert_eq!(records[0]["failed"], true);
    assert_eq!(records[0]["error_stage"], "response_body");
    assert!(!gateway.logs().contains("private-token"));
}
