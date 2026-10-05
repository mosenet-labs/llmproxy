mod support;

use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::{
    ModelMappingInput, ModelRouteInput, ModelRouteTargetInput, ProviderInput, ProviderPaths,
    ProviderStore,
};
use support::*;

#[tokio::test]
async fn chat_route_to_messages_provider_converts_request_and_response() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-cross-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
    let master_key = STANDARD.encode([23; 32]);
    let store = ProviderStore::connect(&url, &master_key).await.unwrap();
    store.migrate().await.unwrap();
    let (upstream, requests) = Mock::http(|request, stream| {
        if request.body.windows(9).any(|part| part == b"malformed") {
            respond(
                stream,
                200,
                "Content-Type: application/json\r\nETag: stale\r\n",
                b"provider-private-invalid-json",
            );
        } else if request.body.windows(9).any(|part| part == b"truncated") {
            use std::io::Write;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 500\r\nConnection: close\r\n\r\n{\"private\":").unwrap();
        } else if request.body.windows(7).any(|part| part == b"timeout") {
            use std::io::Write;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 500\r\n\r\n").unwrap();
            std::thread::sleep(Duration::from_millis(600));
        } else if request.body.windows(6).any(|part| part == b"reject") {
            respond(stream, 429, "Content-Type: application/json\r\n", br#"{"type":"error","error":{"type":"rate_limit_error","message":"provider secret details"}}"#);
        } else if request.body.windows(8).any(|part| part == b"describe") {
            respond(stream, 200, "Content-Type: application/json\r\n", br#"{"type":"message","id":"msg_2","model":"claude-model","role":"assistant","content":[{"type":"text","text":"partial"}],"stop_reason":"max_tokens","stop_sequence":null,"usage":{"input_tokens":3,"cache_read_input_tokens":5,"cache_creation_input_tokens":2,"output_tokens":2}}"#);
        } else {
            respond(
                stream,
                200,
                "Content-Type: application/json\r\n",
                br#"{"type":"message","id":"msg_1","model":"claude-model","role":"assistant","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":2}}"#,
            )
        }
    });
    let mut provider = input("Claude", upstream.address.port(), "provider-secret");
    provider.paths = ProviderPaths::single(Protocol::AnthropicMessages);
    provider.models_protocol = Protocol::AnthropicMessages;
    provider.anthropic_version = Some("2023-06-01".into());
    provider.read_timeout_ms = 250;
    let provider = store.create(provider).await.unwrap();
    let model = store
        .create_model(ModelMappingInput {
            alias: "internal/claude".into(),
            provider_id: provider.id,
            upstream_model_id: "claude-model".into(),
            protocols: vec![Protocol::AnthropicMessages],
            reference_price: None,
        })
        .await
        .unwrap();
    store
        .create_route(ModelRouteInput {
            name: "public-chat".into(),
            protocol: Protocol::OpenAiChat,
            provider_protocol: Protocol::AnthropicMessages,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id: model.id,
                enabled: true,
            }],
        })
        .await
        .unwrap();
    let gateway = Gateway::database(&url, &master_key);
    let response = gateway.request(
        "POST",
        "/v1/chat/completions",
        "Content-Type: application/json\r\nAuthorization: Bearer client-key\r\n",
        br#"{"model":"public-chat","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32}"#,
    );
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&response.body()).unwrap();
    assert_eq!(body["model"], "public-chat");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
    assert_eq!(body["usage"]["prompt_tokens"], 3);
    let request = requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(request.target, "/v1/messages");
    assert_eq!(values(&request.headers, "x-api-key"), ["provider-secret"]);
    assert!(values(&request.headers, "authorization").is_empty());
    // 目标正文在上游发头前已准备，按实际字节长度发送，不再使用 chunked。
    assert_eq!(
        values(&request.headers, "content-length"),
        [request.body.len().to_string()]
    );
    assert!(values(&request.headers, "transfer-encoding").is_empty());
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "claude-model");
    assert_eq!(body["max_tokens"], 32);
    assert_eq!(body["messages"][0]["content"], "hi");
    let failure = gateway.request(
        "POST",
        "/v1/chat/completions",
        "Content-Type: application/json\r\n",
        br#"{"model":"public-chat","messages":[{"role":"user","content":"reject"}],"max_completion_tokens":32}"#,
    );
    assert_eq!(failure.status, 429);
    let body: serde_json::Value = serde_json::from_slice(&failure.body()).unwrap();
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert!(!body.to_string().contains("provider secret details"));
    let _ = requests.recv_timeout(DEADLINE).unwrap();
    let response = gateway.request("POST", "/v1/chat/completions", "Content-Type: application/json\r\n", br#"{"model":"public-chat","messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"data:image/png;base64,YQ=="}}]}],"max_completion_tokens":32,"n":2}"#);
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&response.body()).unwrap();
    assert_eq!(body["choices"][0]["finish_reason"], "length");
    assert_eq!(body["usage"]["prompt_tokens"], 10);
    assert_eq!(body["usage"]["prompt_tokens_details"]["cached_tokens"], 5);
    let request = requests.recv_timeout(DEADLINE).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["messages"][0]["content"][1]["type"], "image");
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["media_type"],
        "image/png"
    );
    assert!(body.get("n").is_none());
    // 上游已经发出 200 后再发生正文错误，客户端仍应得到完整的 502/504。
    for (text, status) in [("malformed", 502), ("truncated", 502), ("timeout", 504)] {
        let input = serde_json::json!({"model":"public-chat","messages":[{"role":"user","content":text}],"max_completion_tokens":32});
        let failed = gateway.request(
            "POST",
            "/v1/chat/completions",
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&input).unwrap(),
        );
        assert_eq!(failed.status, status, "{text}: {}", gateway.logs());
        assert!(values(&failed.headers, "etag").is_empty());
        let body = failed.body();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(error["error"].is_object());
        assert!(!String::from_utf8_lossy(&body).contains("private"));
        requests.recv_timeout(DEADLINE).unwrap();
    }
    // 前缀预读之外的大正文必须完整送入子请求，并且只改写一次模型。
    let long_text = "a".repeat(100_000);
    let input = format!(
        r#"{{"model":"public-chat","messages":[{{"role":"user","content":"{long_text}"}}],"max_completion_tokens":32}}"#
    );
    let response = gateway.request(
        "POST",
        "/v1/chat/completions",
        "Content-Type: application/json\r\n",
        input.as_bytes(),
    );
    assert_eq!(response.status, 200);
    assert!(values(&response.headers, "transfer-encoding").is_empty());
    let length = values(&response.headers, "content-length")[0]
        .parse::<usize>()
        .unwrap();
    assert_eq!(response.body().len(), length);
    let input: serde_json::Value =
        serde_json::from_slice(&requests.recv_timeout(DEADLINE).unwrap().body).unwrap();
    assert_eq!(input["messages"][0]["content"], long_text);
    let completed = gateway
        .logs()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|line| {
            line["fields"]["message"] == "request completed"
                && line["fields"]["route"] == "/v1/chat/completions"
        })
        .count();
    assert_eq!(completed, 7, "父子请求不能重复记录完成统计");

    drop(gateway);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn chat_route_to_gemini_provider_places_model_in_url() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-cross-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
    let master_key = STANDARD.encode([24; 32]);
    let store = ProviderStore::connect(&url, &master_key).await.unwrap();
    store.migrate().await.unwrap();
    let (upstream, requests) = Mock::http(|_, stream| {
        respond(stream, 200, "Content-Type: application/json\r\n", br#"{"responseId":"source","modelVersion":"gemini-model","candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}}"#)
    });
    let mut provider = input("Gemini", upstream.address.port(), "gemini-secret");
    provider.paths = ProviderPaths::single(Protocol::Gemini);
    provider.models_protocol = Protocol::Gemini;
    let provider = store.create(provider).await.unwrap();
    let model = store
        .create_model(ModelMappingInput {
            alias: "internal/gemini".into(),
            provider_id: provider.id,
            upstream_model_id: "gemini-model".into(),
            protocols: vec![Protocol::Gemini],
            reference_price: None,
        })
        .await
        .unwrap();
    store
        .create_route(ModelRouteInput {
            name: "public-chat".into(),
            protocol: Protocol::OpenAiChat,
            provider_protocol: Protocol::Gemini,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id: model.id,
                enabled: true,
            }],
        })
        .await
        .unwrap();
    let gateway = Gateway::database(&url, &master_key);
    let response = gateway.request("POST", "/v1/chat/completions", "Content-Type: application/json\r\n", br#"{"model":"public-chat","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32}"#);
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&response.body()).unwrap();
    assert_eq!(body["model"], "public-chat");
    assert_eq!(body["choices"][0]["message"]["content"], "hello");
    let request = requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        request.target,
        "/v1beta/models/gemini-model:generateContent"
    );
    assert_eq!(
        values(&request.headers, "x-goog-api-key"),
        ["gemini-secret"]
    );
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert!(body.get("model").is_none());
    assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
    drop(gateway);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn sqlite_mixed_provider_rewrites_each_protocol_to_its_configured_path() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-paths-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
    let master_key = STANDARD.encode([19; 32]);
    let store = ProviderStore::connect(&url, &master_key).await.unwrap();
    store.migrate().await.unwrap();
    let (upstream, requests) = Mock::http(|_, stream| respond(stream, 200, "", b"routed"));
    let mut provider = input("Mixed", upstream.address.port(), "shared-test-key");
    provider.paths = ProviderPaths {
        openai_chat: Some("/custom/chat".into()),
        openai_responses: Some("/custom/responses".into()),
        anthropic_messages: Some("/custom/messages".into()),
        gemini: None,
    };
    provider.anthropic_version = Some("2023-06-01".into());
    let record = store.create(provider.clone()).await.unwrap();
    store
        .create_model(ModelMappingInput {
            alias: "public/mock".into(),
            provider_id: record.id,
            upstream_model_id: "upstream-model".into(),
            protocols: vec![
                Protocol::OpenAiChat,
                Protocol::OpenAiResponses,
                Protocol::AnthropicMessages,
            ],
            reference_price: None,
        })
        .await
        .unwrap();
    let gateway = Gateway::database(&url, &master_key);
    for (downstream, upstream_path, auth) in [
        (
            "/v1/chat/completions?stream=1",
            "/custom/chat?stream=1",
            "authorization",
        ),
        ("/v1/responses", "/custom/responses", "authorization"),
        ("/v1/messages", "/custom/messages", "x-api-key"),
    ] {
        assert_eq!(
            gateway
                .request("POST", downstream, "", br#"{"model":"public/mock"}"#)
                .status,
            200
        );
        let request = requests.recv_timeout(DEADLINE).unwrap();
        assert_eq!(request.target, upstream_path);
        assert!(values(&request.headers, auth)[0].contains("shared-test-key"));
        assert_eq!(request.body, br#"{"model":"upstream-model"}"#);
    }
    assert_eq!(
        store.get(record.id).await.unwrap().messages_auth,
        MessagesAuth::ApiKey
    );
    provider.api_key.clear();
    provider.messages_auth = MessagesAuth::Bearer;
    store
        .update(record.id, record.version, provider.clone())
        .await
        .unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        assert_eq!(
            gateway
                .request("POST", "/v1/messages", "", br#"{"model":"public/mock"}"#)
                .status,
            200
        );
        let request = requests.recv_timeout(DEADLINE).unwrap();
        if values(&request.headers, "authorization") == ["Bearer shared-test-key"] {
            assert!(values(&request.headers, "x-api-key").is_empty());
            break;
        }
        assert!(Instant::now() < deadline, "Messages 鉴权切换未生效");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let current = store.get(record.id).await.unwrap();
    let mut edited = provider;
    edited.api_key.clear();
    let saved = store
        .update(record.id, current.version, edited)
        .await
        .unwrap();
    assert_eq!(saved.messages_auth, MessagesAuth::Bearer);
    assert_eq!(gateway.request("POST", PATHS[0], "", b"{}").status, 400);
    assert_eq!(
        gateway
            .request("POST", PATHS[0], "", br#"{"model":"unknown"}"#)
            .status,
        404
    );
    let large = format!(
        r#"{{"model":"public/mock","input":"{}"}}"#,
        "x".repeat(100_000)
    );
    assert_eq!(
        gateway
            .request("POST", PATHS[0], "", large.as_bytes())
            .status,
        200
    );
    let request = requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        request.body,
        large.replace("public/mock", "upstream-model").as_bytes()
    );
    let late = format!(
        r#"{{"input":"{}","model":"public/mock"}}"#,
        "x".repeat(66_000)
    );
    assert_eq!(
        gateway
            .request("POST", PATHS[0], "", late.as_bytes())
            .status,
        413
    );
    let current = store.get(record.id).await.unwrap();
    store
        .set_enabled(record.id, current.version, false)
        .await
        .unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        let status = gateway
            .request("POST", PATHS[0], "", br#"{"model":"public/mock"}"#)
            .status;
        if status == 503 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disabled mapping did not become unavailable"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(gateway);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn gemini_native_requests_rewrite_model_path_and_stream_without_buffering() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-gemini-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
    let master_key = STANDARD.encode([21; 32]);
    let store = ProviderStore::connect(&url, &master_key).await.unwrap();
    store.migrate().await.unwrap();
    let (release, acknowledged) = mpsc::channel();
    let acknowledged = Arc::new(Mutex::new(acknowledged));
    let (upstream, requests) = Mock::http(move |request, stream| {
        if request.target.contains("streamGenerateContent") {
            sse_headers(stream);
            chunk(
                stream,
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]}}]}\n\n",
            )
            .unwrap();
            acknowledged.lock().unwrap().recv_timeout(DEADLINE).unwrap();
            chunk(stream, b"data: {\"candidates\":[]}\n\n").unwrap();
            finish_chunks(stream);
        } else {
            respond(
                stream,
                200,
                "Content-Type: application/json\r\n",
                br#" {"candidates":[{"content":{"parts":[{"text":"hello"}]}}]} "#,
            );
        }
    });
    let mut provider = input("Google", upstream.address.port(), "gemini-secret");
    provider.paths = ProviderPaths::single(Protocol::Gemini);
    provider.models_path = "/v1beta/models".into();
    provider.models_protocol = Protocol::Gemini;
    let record = store.create(provider).await.unwrap();
    store
        .create_model(ModelMappingInput {
            alias: "public/gemini".into(),
            provider_id: record.id,
            upstream_model_id: "gemini-test".into(),
            protocols: vec![Protocol::Gemini],
            reference_price: None,
        })
        .await
        .unwrap();
    let gateway = Gateway::database(&url, &master_key);
    let body = br#"{"contents":[{"parts":[{"text":"hi"}]}]}"#;
    let response = gateway.request(
        "POST",
        "/v1beta/models/public%2Fgemini:generateContent",
        "",
        body,
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body(),
        br#" {"candidates":[{"content":{"parts":[{"text":"hello"}]}}]} "#
    );
    let received = requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        received.target,
        "/v1beta/models/gemini-test:generateContent"
    );
    assert_eq!(received.body, body);
    assert_eq!(
        values(&received.headers, "x-goog-api-key"),
        ["gemini-secret"]
    );
    assert!(values(&received.headers, "authorization").is_empty());

    let mut response = gateway.request(
        "POST",
        "/v1beta/models/public%2Fgemini:streamGenerateContent",
        "",
        body,
    );
    assert_eq!(response.status, 200);
    let first = b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]}}]}\n\n";
    assert_eq!(response.bytes(first.len()), first);
    release.send(()).unwrap();
    assert_eq!(response.body(), b"data: {\"candidates\":[]}\n\n");
    let received = requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        received.target,
        "/v1beta/models/gemini-test:streamGenerateContent?alt=sse"
    );
    assert_eq!(received.body, body);
    assert_eq!(
        values(&received.headers, "x-goog-api-key"),
        ["gemini-secret"]
    );
    drop(gateway);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

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
        paths: ProviderPaths::single(Protocol::OpenAiChat),
        host: "127.0.0.1".to_owned(),
        port,
        tls: false,
        api_key: secret.to_owned(),
        enabled: true,
        models_path: "/models".into(),
        models_protocol: Protocol::OpenAiChat,
        anthropic_version: None,
        messages_auth: MessagesAuth::ApiKey,
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
        assert_eq!(
            gateway
                .request("POST", path, "", br#"{"model":"public/model"}"#)
                .status,
            404
        );
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
    let mapping = store
        .create_model(ModelMappingInput {
            alias: "public/model".into(),
            provider_id: old.id,
            upstream_model_id: "upstream-old".into(),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
        })
        .await
        .unwrap();
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
        br#"{"model":"public/model"}"#,
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
    assert!(response.text().await.unwrap().contains("Model Routes"));

    let new = store
        .create(input(
            "New provider",
            new_upstream.address.port(),
            "new-dummy-key",
        ))
        .await
        .unwrap();
    store
        .update_model(
            mapping.id,
            mapping.version,
            ModelMappingInput {
                alias: "public/model".into(),
                provider_id: new.id,
                upstream_model_id: "upstream-new".into(),
                protocols: vec![Protocol::OpenAiChat],
                reference_price: None,
            },
        )
        .await
        .unwrap();
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
    let response = gateway.request("POST", PATHS[0], "", br#"{"model":"public/model"}"#);
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
    assert_eq!(
        gateway
            .request("POST", PATHS[0], "", br#"{"model":"public/model"}"#)
            .status,
        503
    );
    assert_eq!(new_upstream.count(), count);
    assert!(
        store
            .load_model_routes()
            .await
            .unwrap()
            .iter()
            .all(|route| !route.enabled)
    );
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
        let response = gateway.request("POST", PATHS[0], "", br#"{"model":"public/model"}"#);
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
