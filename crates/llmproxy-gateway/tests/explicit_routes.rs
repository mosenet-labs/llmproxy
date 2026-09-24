mod support;

use std::{
    io::{self, Read, Write},
    net::{Shutdown, ToSocketAddrs},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use support::*;

#[test]
fn proxies_requests_headers_and_provider_errors_for_each_route() {
    let upstreams: Vec<_> = (0..3)
        .map(|_| Mock::http(|request, stream| {
            let status: u16 = request.target.split("status=").nth(1).unwrap().split('&').next().unwrap().parse().unwrap();
            let body = format!("{{ \"upstream_status\": {status}, \"error\": \"原始内容\\n\" }}\n");
            respond(stream, status, "Content-Type: application/problem+json\r\nRetry-After: 7\r\nRequest-Id: provider-request-id\r\nX-Request-Id: provider-x-request-id\r\nConnection: x-upstream-only\r\nX-Upstream-Only: secret\r\nKeep-Alive: timeout=300\r\n", body.as_bytes());
        }))
        .collect();
    let gateway = Gateway::start(std::array::from_fn(|index| {
        Provider::http(upstreams[index].0.address)
    }));
    let body = b"{  \"model\":\"mock\",\"messages\":[] , \"input\":\"raw\" }\n";
    for (index, path) in PATHS.iter().enumerate() {
        for status in [200, 400, 401, 429, 500, 503] {
            let target = format!("{path}?status={status}&encoded=%2F%20&tag=a&tag=b");
            let response = gateway.request(
                "POST",
                &target,
                concat!(
                    "Authorization: Bearer caller-one\r\n",
                    "aUtHoRiZaTiOn: Bearer caller-two\r\n",
                    "X-Api-Key: caller-key-one\r\n",
                    "x-api-key: caller-key-two\r\n",
                    "Content-Type: application/json; charset=utf-8\r\n",
                    "Accept: text/event-stream\r\n",
                    "anthropic-version: client-version\r\n",
                    "anthropic-beta: test-beta\r\n",
                ),
                body,
            );
            assert_eq!(response.status, status, "{path}");
            assert_eq!(
                values(&response.headers, "content-type"),
                ["application/problem+json"]
            );
            assert_eq!(values(&response.headers, "retry-after"), ["7"]);
            assert!(values(&response.headers, "x-upstream-only").is_empty());
            assert!(values(&response.headers, "keep-alive").is_empty());
            assert_eq!(
                values(&response.headers, "request-id"),
                ["provider-request-id"]
            );
            assert_eq!(
                values(&response.headers, "x-request-id"),
                ["provider-x-request-id"]
            );
            assert_eq!(
                response.body(),
                format!("{{ \"upstream_status\": {status}, \"error\": \"原始内容\\n\" }}\n")
                    .as_bytes()
            );
            let received = upstreams[index].1.recv_timeout(DEADLINE).unwrap();
            assert_eq!(received.method, "POST");
            assert_eq!(received.target, target);
            assert_eq!(received.body, body);
            assert_eq!(
                values(&received.headers, "host"),
                [upstreams[index].0.address.to_string()]
            );
            assert_eq!(
                values(&received.headers, "content-type"),
                ["application/json; charset=utf-8"]
            );
            assert_eq!(values(&received.headers, "accept"), ["text/event-stream"]);
            assert_eq!(values(&received.headers, "anthropic-beta"), ["test-beta"]);
            if index == 2 {
                assert!(values(&received.headers, "authorization").is_empty());
                assert_eq!(values(&received.headers, "x-api-key"), [SECRETS[index]]);
                assert_eq!(
                    values(&received.headers, "anthropic-version"),
                    ["2023-06-01"]
                );
            } else {
                assert_eq!(
                    values(&received.headers, "authorization"),
                    [format!("Bearer {}", SECRETS[index])]
                );
                assert!(values(&received.headers, "x-api-key").is_empty());
                assert_eq!(
                    values(&received.headers, "anthropic-version"),
                    ["client-version"]
                );
            }
        }
    }
    for (mock, received) in &upstreams {
        assert_eq!(mock.count(), 6, "no extra upstream attempt");
        assert!(received.try_recv().is_err());
    }
}

#[test]
fn preserves_client_anthropic_version_without_config_override() {
    let (upstream, received) = Mock::http(|_, stream| respond(stream, 200, "", b"ok"));
    let mut provider = Provider::http(upstream.address);
    provider.version = None;
    let gateway = Gateway::start([provider; 3]);
    assert_eq!(
        gateway
            .request(
                "POST",
                PATHS[2],
                "anthropic-version: client-version\r\n",
                b"{}"
            )
            .body(),
        b"ok"
    );
    assert_eq!(
        values(
            &received.recv_timeout(DEADLINE).unwrap().headers,
            "anthropic-version"
        ),
        ["client-version"]
    );
    assert_eq!(gateway.request("POST", PATHS[2], "", b"{}").status, 200);
    assert!(
        values(
            &received.recv_timeout(DEADLINE).unwrap().headers,
            "anthropic-version"
        )
        .is_empty()
    );
}

#[test]
fn rejects_connection_options_that_conflict_with_response_framing() {
    let (upstream, requests) = Mock::http(|request, stream| {
        let option = request.target.split_once('?').unwrap().1;
        respond(stream, 200, &format!("Connection: {option}\r\n"), b"ok");
    });
    let gateway = Gateway::start([Provider::http(upstream.address); 3]);
    for option in ["content-length", "transfer-encoding", "content-encoding"] {
        let response = gateway.request("POST", &format!("{}?{option}", PATHS[0]), "", b"{}");
        assert_eq!(response.status, 502);
        requests.recv_timeout(DEADLINE).unwrap();
    }
    assert_eq!(upstream.count(), 3);
}

#[test]
fn local_rejections_never_contact_a_provider() {
    let (upstream, received) = Mock::http(|_, stream| respond(stream, 599, "", b"unexpected"));
    let gateway = Gateway::start([Provider::http(upstream.address); 3]);
    for path in ["/unknown", "/v1/messages/", "/v1/chat/completions-extra"] {
        assert_eq!(gateway.request("POST", path, "", b"{}").status, 404);
    }
    for path in PATHS.into_iter().chain(["/v1/auto"]) {
        for method in ["GET", "PUT", "OPTIONS"] {
            let response = gateway.request(method, path, "", b"");
            assert_eq!(response.status, 405);
            assert_eq!(values(&response.headers, "allow"), ["POST"]);
        }
    }
    assert_eq!(gateway.request("POST", "/v1/auto", "", b"{}").status, 501);
    assert_eq!(upstream.count(), 0);
    assert!(received.try_recv().is_err());
}

#[test]
fn streams_each_protocol_before_upstream_finishes() {
    let events: [(&[u8], &[u8]); 3] = [
        (
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n",
        ),
        (
            b"event: response.output_text.delta\ndata: {\"delta\":\"hello\"}\n\n",
            b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        ),
        (
            b"event: content_block_delta\ndata: {\"delta\":{\"text\":\"hello\"}}\n\n",
            b"event: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
        ),
    ];
    for (index, (first, remaining)) in events.into_iter().enumerate() {
        let (release, acknowledged) = mpsc::channel();
        let acknowledged = Arc::new(Mutex::new(acknowledged));
        let (upstream, requests) = Mock::http(move |_, stream| {
            sse_headers(stream);
            // Split inside an event; later combine multiple events in one chunk.
            chunk(stream, &first[..13]).unwrap();
            chunk(stream, &first[13..]).unwrap();
            acknowledged.lock().unwrap().recv_timeout(DEADLINE).unwrap();
            chunk(stream, remaining).unwrap();
            finish_chunks(stream);
        });
        let gateway = Gateway::start([Provider::http(upstream.address); 3]);
        let mut response = gateway.request(
            "POST",
            PATHS[index],
            "Accept: text/event-stream\r\n",
            b"{\"stream\":true}",
        );
        assert_eq!(response.status, 200);
        assert_eq!(
            values(&response.headers, "content-type"),
            ["text/event-stream"]
        );
        assert!(values(&response.headers, "content-encoding").is_empty());
        assert_eq!(response.bytes(first.len()), first);
        release.send(()).unwrap();
        assert_eq!(response.body(), remaining);
        requests.recv_timeout(DEADLINE).unwrap();
        assert_eq!(upstream.count(), 1);
    }
}

#[test]
fn active_sse_outlives_the_read_timeout() {
    let (stop, wait) = mpsc::channel::<()>();
    let wait = Arc::new(Mutex::new(wait));
    let (upstream, _) = Mock::http(move |_, stream| {
        sse_headers(stream);
        for _ in 0..5 {
            chunk(stream, b"data: heartbeat\n\n").unwrap();
            match wait
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_millis(350))
            {
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                _ => return,
            }
        }
        finish_chunks(stream);
    });
    let mut provider = Provider::http(upstream.address);
    provider.read_ms = 1100;
    let gateway = Gateway::start([provider; 3]);
    let start = Instant::now();
    let response = gateway.request("POST", PATHS[0], "", b"{}");
    assert_eq!(response.status, 200);
    assert_eq!(response.body(), b"data: heartbeat\n\n".repeat(5));
    assert!(start.elapsed() > Duration::from_millis(provider.read_ms));
    drop(stop);
    assert_eq!(upstream.count(), 1);
}

#[test]
fn response_header_timeout_is_504_and_never_retries() {
    let (release, hold) = mpsc::channel::<()>();
    let hold = Arc::new(Mutex::new(hold));
    let (upstream, requests) = Mock::http(move |_, _| {
        let _ = hold.lock().unwrap().recv_timeout(DEADLINE);
    });
    let mut provider = Provider::http(upstream.address);
    provider.read_ms = 700;
    let gateway = Gateway::start([provider; 3]);
    for path in PATHS {
        let started = Instant::now();
        let response = gateway.request("POST", path, "", b"{}");
        assert_eq!(response.status, 504, "{}", gateway.logs());
        assert!(started.elapsed() < DEADLINE);
        assert!(started.elapsed() >= Duration::from_millis(provider.read_ms));
        requests.recv_timeout(DEADLINE).unwrap();
    }
    assert_eq!(upstream.count(), 3);
    drop(release);
}

#[test]
fn sse_timeout_or_upstream_disconnect_truncates_without_a_second_response() {
    for disconnect in [false, true] {
        let (release, hold) = mpsc::channel::<()>();
        let hold = Arc::new(Mutex::new(hold));
        let (upstream, requests) = Mock::http(move |_, stream| {
            sse_headers(stream);
            chunk(stream, b"data: partial\n\n").unwrap();
            if !disconnect {
                let _ = hold.lock().unwrap().recv_timeout(DEADLINE);
            }
            // No terminating HTTP chunk and no protocol completion event.
        });
        let mut provider = Provider::http(upstream.address);
        provider.read_ms = 700;
        let gateway = Gateway::start([provider; 3]);
        for path in PATHS {
            let started = Instant::now();
            let mut response = gateway.request("POST", path, "", b"{}");
            assert_eq!(response.status, 200);
            assert_eq!(
                response.bytes(b"data: partial\n\n".len()),
                b"data: partial\n\n"
            );
            assert_eq!(values(&response.headers, "transfer-encoding"), ["chunked"]);
            let error = response
                .next_chunk()
                .expect_err("failed SSE must not end with a valid zero chunk or appended body");
            assert!(
                matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                ),
                "{error}"
            );
            if !disconnect {
                assert!(started.elapsed() >= Duration::from_millis(provider.read_ms));
            }
            requests.recv_timeout(DEADLINE).unwrap();
        }
        assert_eq!(upstream.count(), 3, "failed POST was retried");
        if disconnect {
            let deadline = Instant::now() + DEADLINE;
            while !gateway
                .logs()
                .contains("\"protocol\":\"anthropic_messages\"")
            {
                assert!(
                    Instant::now() < deadline,
                    "missing completion log: {}",
                    gateway.logs()
                );
                thread::sleep(Duration::from_millis(20));
            }
            let logs = gateway.logs();
            assert!(
                !logs.contains("ReadTimedout"),
                "disconnect became a timeout: {logs}"
            );
            assert!(
                logs.contains("\"error_type\":\"ReadError\"")
                    || logs.contains("\"error_type\":\"ConnectionClosed\""),
                "expected a transport error after upstream disconnect: {logs}"
            );
        }
        drop(release);
    }
}

#[test]
fn client_cancellation_closes_the_upstream_stream() {
    let (release, cancelled) = mpsc::channel();
    let cancelled = Arc::new(Mutex::new(cancelled));
    let (closed, closure) = mpsc::channel();
    let (upstream, _) = Mock::http(move |_, stream| {
        sse_headers(stream);
        chunk(stream, b"data: first\n\n").unwrap();
        cancelled.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let deadline = Instant::now() + DEADLINE;
        let data = vec![b'x'; 64 * 1024];
        while Instant::now() < deadline {
            if let Err(error) = chunk(stream, &data) {
                assert!(
                    matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::NotConnected
                    ),
                    "upstream did not close after cancellation: {error}"
                );
                closed.send(()).unwrap();
                return;
            }
        }
        panic!("gateway did not cancel the upstream stream");
    });
    let gateway = Gateway::start([Provider::http(upstream.address); 3]);
    let mut response = gateway.request("POST", PATHS[2], "", b"{}");
    assert_eq!(response.bytes(13), b"data: first\n\n");
    response.cancel();
    release.send(()).unwrap();
    closure.recv_timeout(DEADLINE).unwrap();
    assert_eq!(upstream.count(), 1);
}

#[test]
fn connection_refusal_is_502_and_gateway_remains_available() {
    let reserved = bind_listener("127.0.0.1:0");
    let address = reserved.local_addr().unwrap();
    let gateway = Gateway::start([Provider::http(address); 3]);
    drop(reserved);
    for path in PATHS {
        assert_eq!(gateway.request("POST", path, "", b"{}").status, 502);
    }
    assert_eq!(gateway.request("POST", "/unknown", "", b"{}").status, 404);
}

#[test]
fn tls_client_hello_has_hostname_sni_and_handshake_timeout_is_504() {
    let address = ("localhost", 0).to_socket_addrs().unwrap().next().unwrap();
    let listener = bind_listener(address);
    let (sent, received) = mpsc::channel();
    let (release, hold) = mpsc::channel::<()>();
    let hold = Arc::new(Mutex::new(hold));
    let upstream = Mock::on_listener(listener, move |mut stream| {
        let mut record = [0; 5];
        stream.read_exact(&mut record).unwrap();
        assert_eq!(record[0], 22, "TLS handshake record");
        let mut hello = vec![0; u16::from_be_bytes([record[3], record[4]]) as usize];
        stream.read_exact(&mut hello).unwrap();
        sent.send(client_hello_sni(&hello)).unwrap();
        let _ = hold.lock().unwrap().recv_timeout(DEADLINE);
    });
    let mut provider = Provider::http(upstream.address);
    provider.tls = true;
    provider.connect_ms = 700;
    let gateway = Gateway::start([provider; 3]);
    let started = Instant::now();
    assert_eq!(
        gateway.request("POST", PATHS[0], "", b"{}").status,
        504,
        "{}",
        gateway.logs()
    );
    assert_eq!(received.recv_timeout(DEADLINE).unwrap(), "localhost");
    assert!(started.elapsed() < DEADLINE);
    assert!(started.elapsed() >= Duration::from_millis(provider.connect_ms));
    assert_eq!(upstream.count(), 1);
    drop(release);
}

fn client_hello_sni(bytes: &[u8]) -> String {
    assert_eq!(bytes[0], 1, "ClientHello handshake");
    let mut offset = 4 + 2 + 32;
    offset += 1 + bytes[offset] as usize;
    offset += 2 + u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
    offset += 1 + bytes[offset] as usize;
    let end = offset + 2 + u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
    offset += 2;
    while offset < end {
        let kind = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        let length = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        offset += 4;
        if kind == 0 {
            assert_eq!(bytes[offset + 2], 0, "host_name SNI type");
            let size = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as usize;
            return String::from_utf8(bytes[offset + 5..offset + 5 + size].to_vec()).unwrap();
        }
        offset += length;
    }
    panic!("ClientHello has no SNI");
}

#[test]
fn stalled_upstream_request_write_is_504_without_retry() {
    let (accepted, connection) = mpsc::channel();
    let (release, hold) = mpsc::channel::<()>();
    let hold = Arc::new(Mutex::new(hold));
    let upstream = Mock::raw(move |_stream| {
        accepted.send(()).unwrap();
        // Accept TCP without reading HTTP, forcing bounded socket buffers to fill.
        let _ = hold.lock().unwrap().recv_timeout(DEADLINE);
    });
    let mut provider = Provider::http(upstream.address);
    provider.write_ms = 700;
    provider.read_ms = 2500;
    let gateway = Gateway::start([provider; 3]);
    let mut stream = gateway.connect();
    write!(stream, "POST {} HTTP/1.1\r\nHost: client.invalid\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", PATHS[1], 128 * 1024 * 1024).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let sending = thread::spawn(move || {
        let block = vec![b'x'; 64 * 1024];
        for _ in 0..2048 {
            if writer.write_all(&block).is_err() {
                break;
            }
        }
    });
    connection.recv_timeout(DEADLINE).unwrap();
    let started = Instant::now();
    let response = Response::read(stream.try_clone().unwrap()).unwrap();
    assert_eq!(response.status, 504, "{}", gateway.logs());
    // Pingora waits for a possible upstream error response after a write failure.
    assert!(started.elapsed() < Duration::from_secs(5));
    let _ = stream.shutdown(Shutdown::Both);
    sending.join().unwrap();
    release.send(()).unwrap();
    assert_eq!(upstream.count(), 1);
    let deadline = Instant::now() + DEADLINE;
    while !gateway.logs().contains("WriteTimedout") {
        assert!(
            Instant::now() < deadline,
            "expected upstream write timeout: {}",
            gateway.logs()
        );
        thread::sleep(Duration::from_millis(20));
    }
}
