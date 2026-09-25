mod support;

use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ProviderInput, ProviderStore};
use support::*;

#[tokio::test]
async fn database_updates_new_requests_without_interrupting_existing_sse() {
    let Ok(base_url) = std::env::var("LLMPROXY_TEST_DATABASE_URL") else {
        eprintln!("跳过 Gateway PostgreSQL 集成测试：未设置 LLMPROXY_TEST_DATABASE_URL");
        return;
    };
    let mut admin = toasty::Db::builder()
        .connect(&base_url)
        .await
        .unwrap_or_else(|_| panic!("无法连接 LLMPROXY_TEST_DATABASE_URL"));
    let schema = format!(
        "llmproxy_gateway_test_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    toasty::sql::statement(format!("CREATE SCHEMA {schema}"))
        .exec(&mut admin)
        .await
        .unwrap_or_else(|_| panic!("无法创建网关独立测试 schema"));
    let separator = if base_url.contains('?') { '&' } else { '?' };
    let url = format!("{base_url}{separator}options=-c%20search_path%3D{schema}");
    // Catch test panics before dropping our isolated schema. No application
    // tables are changed, even when an assertion in the worker fails.
    let worker = tokio::spawn(async move { exercise_gateway(&url).await });
    let result = worker.await;
    toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
        .exec(&mut admin)
        .await
        .unwrap_or_else(|_| panic!("无法清理网关测试 schema：{schema}"));
    result.unwrap();
}

fn input(name: &str, port: u16, secret: &str) -> ProviderInput {
    ProviderInput {
        name: name.to_owned(),
        protocol: Protocol::OpenAiChat,
        host: "127.0.0.1".to_owned(),
        port,
        tls: false,
        api_key: secret.to_owned(),
        enabled: true,
        anthropic_version: None,
        connect_timeout_ms: 2000,
        read_timeout_ms: 15_000,
        write_timeout_ms: 2000,
    }
}

async fn exercise_gateway(url: &str) {
    let master_key = STANDARD.encode([19; 32]);
    let store = ProviderStore::connect(url, &master_key).await.unwrap();
    store.migrate().await.unwrap();
    let (release, next_event) = mpsc::channel();
    let next_event = Arc::new(Mutex::new(next_event));
    let (old_upstream, old_requests) = Mock::http(move |request, stream| {
        if request.target.ends_with("?stream=1") {
            sse_headers(stream);
            chunk(stream, b"data: old-provider-first\n\n").unwrap();
            next_event.lock().unwrap().recv_timeout(DEADLINE).unwrap();
            chunk(stream, b"data: old-provider-last\n\ndata: [DONE]\n\n").unwrap();
            finish_chunks(stream);
        } else {
            respond(stream, 200, "", b"old-provider");
        }
    });
    let (new_upstream, new_requests) = Mock::http(|request, stream| {
        let authorization = values(&request.headers, "authorization")[0];
        let host = values(&request.headers, "host")[0];
        respond(
            stream,
            200,
            &format!("X-Mock-Authorization: {authorization}\r\nX-Mock-Host: {host}\r\n"),
            b"new-provider",
        );
    });
    let gateway = Gateway::database(url, &master_key);

    for path in PATHS {
        assert_eq!(gateway.request("POST", path, "", b"{}").status, 503);
    }
    assert_eq!(gateway.request("POST", "/v1/auto", "", b"{}").status, 501);
    assert_eq!(old_upstream.count(), 0);
    assert_eq!(new_upstream.count(), 0);

    let old = store
        .create(input(
            "Old provider",
            old_upstream.address.port(),
            "old-dummy-key",
        ))
        .await
        .unwrap();
    store.activate(old.id, old.version).await.unwrap();
    wait_for_route(&gateway, 200, Some(b"old-provider"), None).await;
    let first = old_requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        values(&first.headers, "host"),
        [old_upstream.address.to_string()]
    );
    assert_eq!(
        values(&first.headers, "authorization"),
        ["Bearer old-dummy-key"]
    );

    let mut in_flight = gateway.request(
        "POST",
        &format!("{}?stream=1", PATHS[0]),
        "Accept: text/event-stream\r\n",
        b"{}",
    );
    assert_eq!(in_flight.status, 200);
    assert_eq!(
        in_flight.bytes(b"data: old-provider-first\n\n".len()),
        b"data: old-provider-first\n\n"
    );
    let stream_request = old_requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        values(&stream_request.headers, "authorization"),
        ["Bearer old-dummy-key"]
    );

    // A pending SSE must not block the console sharing this listener/runtime.
    let response = reqwest::Client::new()
        .get(format!("http://{}/ui/routes", gateway.address))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.text().await.unwrap().contains("路由概览"));

    let new = store
        .create(input(
            "New provider",
            new_upstream.address.port(),
            "new-dummy-key",
        ))
        .await
        .unwrap();
    store.activate(new.id, new.version).await.unwrap();
    wait_for_route(
        &gateway,
        200,
        Some(b"new-provider"),
        Some("Bearer new-dummy-key"),
    )
    .await;
    let selected = new_requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        values(&selected.headers, "host"),
        [new_upstream.address.to_string()]
    );
    assert_eq!(
        values(&selected.headers, "authorization"),
        ["Bearer new-dummy-key"]
    );

    // Only release the old upstream after a new request demonstrably used the
    // replacement. The existing stream must continue on the original socket.
    release.send(()).unwrap();
    assert_eq!(
        in_flight.body(),
        b"data: old-provider-last\n\ndata: [DONE]\n\n"
    );
    assert!(
        new_requests
            .try_iter()
            .all(|request| !request.target.contains("stream=1"))
    );

    // Editing the active provider also refreshes credentials for new requests.
    let current = store.get(new.id).await.unwrap();
    store
        .update(
            new.id,
            current.version,
            input(
                "New provider",
                new_upstream.address.port(),
                "rotated-dummy-key",
            ),
        )
        .await
        .unwrap();
    wait_for_route(
        &gateway,
        200,
        Some(b"new-provider"),
        Some("Bearer rotated-dummy-key"),
    )
    .await;

    // A transaction in this test's private schema holds the route rows, causing
    // the background load to exceed its deadline. No database is stopped and no
    // production data or table definition is changed.
    let mut inspection = toasty::Db::builder().connect(url).await.unwrap();
    let mut lock = inspection.transaction().await.unwrap();
    toasty::sql::query("SELECT protocol FROM route_bindings ORDER BY protocol FOR UPDATE")
        .exec(&mut lock)
        .await
        .unwrap();
    wait_for_log(
        &gateway,
        "provider snapshot refresh failed; retaining the last snapshot",
    )
    .await;
    let response = gateway.request("POST", PATHS[0], "", b"{}");
    assert_eq!(response.status, 200);
    assert_eq!(
        values(&response.headers, "x-mock-authorization"),
        ["Bearer rotated-dummy-key"]
    );
    assert_eq!(response.body(), b"new-provider");
    lock.rollback().await.unwrap();
    wait_for_log(&gateway, "provider snapshot refresh recovered").await;

    let current = store.get(new.id).await.unwrap();
    store
        .set_enabled(new.id, current.version, false)
        .await
        .unwrap();
    wait_for_route(&gateway, 503, None, None).await;
    let count = new_upstream.count();
    assert_eq!(gateway.request("POST", PATHS[0], "", b"{}").status, 503);
    assert_eq!(new_upstream.count(), count);
    assert!(store.load_active().await.unwrap().is_empty());
    let logs = gateway.logs();
    for secret in [
        "old-dummy-key",
        "new-dummy-key",
        "rotated-dummy-key",
        master_key.as_str(),
        url,
    ] {
        assert!(
            !logs.contains(secret),
            "gateway logs must not contain credentials"
        );
    }
}

async fn wait_for_route(
    gateway: &Gateway,
    status: u16,
    body: Option<&[u8]>,
    authorization: Option<&str>,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        // Every poll is an independent new request observing snapshot convergence.
        let response = gateway.request("POST", PATHS[0], "", b"{}");
        let matched = response.status == status
            && authorization.is_none_or(|expected| {
                values(&response.headers, "x-mock-authorization") == [expected]
            });
        let actual_body = response.body();
        if matched && body.is_none_or(|expected| actual_body == expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "gateway snapshot did not converge: {}",
            gateway.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_log(gateway: &Gateway, message: &str) {
    let deadline = Instant::now() + DEADLINE;
    while !gateway.logs().contains(message) {
        assert!(
            Instant::now() < deadline,
            "expected {message}: {}",
            gateway.logs()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
