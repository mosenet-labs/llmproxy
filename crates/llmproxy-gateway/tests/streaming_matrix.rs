mod nonstream;
mod streaming;
mod support;
use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::{ProviderInput, ProviderPaths};
use nonstream::{ALL, Database, MASTER_KEY, alias, fixtures};
use std::sync::{Arc, Mutex, mpsc};
use support::{DEADLINE, Gateway, Mock, chunk, finish_chunks, sse_headers, values};

#[tokio::test]
async fn signed_stream_tools_survive_gateway_restart() {
    let (upstream, _) = Mock::http(|request, socket| {
        let input: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let result = input["contents"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|content| content["parts"].as_array().unwrap())
            .any(|part| part.get("functionResponse").is_some());
        if result {
            let call = input["contents"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|content| content["parts"].as_array().unwrap())
                .find(|part| part.get("functionCall").is_some())
                .unwrap();
            assert_eq!(call["thoughtSignature"], "private-signature");
            assert_eq!(call["functionCall"]["id"], "call_1");
        }
        sse_headers(socket);
        let (start, end) = streaming::frames(Protocol::Gemini, !result);
        for frame in start.into_iter().chain(end) {
            chunk(socket, &frame).unwrap();
        }
        finish_chunks(socket);
    });
    let database = Database::new().await;
    database
        .add_provider(
            streaming::provider(Protocol::Gemini, upstream.address.port(), 3000),
            Protocol::Gemini,
            "m",
        )
        .await;
    for client in [
        Protocol::OpenAiChat,
        Protocol::OpenAiResponses,
        Protocol::AnthropicMessages,
    ] {
        let alias = alias(client, Protocol::Gemini);
        let mut input = fixtures::tool_request(client, &alias);
        input["stream"] = serde_json::json!(true);
        let first = Gateway::database(&database.url, MASTER_KEY);
        let response = first.request(
            "POST",
            &streaming::path(client, &alias),
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&input).unwrap(),
        );
        assert_eq!(response.status, 200);
        let mut observed = streaming::Observed::new(client);
        observed.push(client, &response.body());
        observed.finish(client);
        let (id, name, arguments) = observed.tools.values().next().unwrap();
        let call = llmproxy_core::ir::message::ToolCall {
            id: id.clone(),
            name: name.clone(),
            arguments: serde_json::from_str(arguments).unwrap(),
        };
        let mut output = llmproxy_core::ir::response::Response::new(client);
        output
            .items
            .push(llmproxy_core::ir::response::Item::ToolCall {
                call: call.clone(),
                item_id: None,
            });
        let next = fixtures::tool_result_request(client, &alias, &input, &output, &call).unwrap();
        streaming::assert_logs(&first, 1);
        drop(first);
        let restarted = Gateway::database(&database.url, MASTER_KEY);
        let response = restarted.request(
            "POST",
            &streaming::path(client, &alias),
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&next).unwrap(),
        );
        assert_eq!(response.status, 200);
        let mut followup = streaming::Observed::new(client);
        followup.push(client, &response.body());
        followup.finish(client);
        assert_eq!(followup.text, "你好");
        streaming::assert_logs(&restarted, 1);
    }
}

#[tokio::test]
async fn multiple_candidates_remain_separate_or_select_zero_with_a_warning() {
    use serde_json::json;
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for provider in [Protocol::OpenAiChat, Protocol::Gemini] {
        let (upstream, _) = Mock::http(move |_, socket| {
            sse_headers(socket);
            let frames = if provider == Protocol::OpenAiChat {
                vec![
                    json!({"id":"raw","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"zero"}},{"index":1,"delta":{"role":"assistant","content":"one"}}]}),
                    json!({"id":"raw","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"},{"index":1,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}}),
                ]
            } else {
                vec![
                    json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"zero"}]}},{"index":1,"content":{"role":"model","parts":[{"text":"one"}]}}]}),
                    json!({"candidates":[{"index":0,"finishReason":"STOP"},{"index":1,"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13}}),
                ]
            };
            for frame in frames {
                chunk(socket, format!("data: {frame}\n\n").as_bytes()).unwrap();
            }
            if provider == Protocol::OpenAiChat {
                chunk(socket, b"data: [DONE]\n\n").unwrap();
            }
            finish_chunks(socket);
        });
        database
            .add_provider(
                streaming::provider(provider, upstream.address.port(), 3000),
                provider,
                "m",
            )
            .await;
        upstreams.push(upstream);
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for provider in [Protocol::OpenAiChat, Protocol::Gemini] {
        for client in ALL {
            let alias = alias(client, provider);
            let mut input = fixtures::request(client, &alias, "candidates");
            if client != Protocol::Gemini {
                input["stream"] = json!(true);
            }
            let response = gateway.request(
                "POST",
                &streaming::path(client, &alias),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&input).unwrap(),
            );
            assert_eq!(response.status, 200);
            let mut observed = streaming::Observed::new(client);
            observed.push(client, &response.body());
            observed.finish(client);
            assert_eq!(observed.candidates.get(&0).unwrap(), "zero");
            if matches!(client, Protocol::OpenAiChat | Protocol::Gemini) {
                assert_eq!(observed.candidates.get(&1).unwrap(), "one");
            } else {
                assert_eq!(observed.candidates.len(), 1);
            }
            assert_eq!(
                observed
                    .decoder
                    .state()
                    .usage()
                    .snapshot()
                    .unwrap()
                    .total_tokens,
                Some(13)
            );
        }
    }
    streaming::assert_logs(&gateway, 8);
    assert!(
        gateway.logs().contains("conversion"),
        "单候选目标应记录丢弃告警"
    );
}

#[tokio::test]
async fn sixteen_directions_deliver_text_before_upstream_finishes() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (release, ready) = mpsc::channel();
        let ready = Arc::new(Mutex::new(ready));
        let (upstream, requests) = Mock::http(move |request, socket| {
            sse_headers(socket);
            let body = String::from_utf8_lossy(&request.body);
            let result = [
                "tool_call_id",
                "tool_result",
                "function_call_output",
                "functionResponse",
            ]
            .iter()
            .any(|field| body.contains(field));
            let tool = !result && (body.contains("tools") || body.contains("functionDeclarations"));
            if result && target == Protocol::Gemini {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let part = body["contents"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|content| content["parts"].as_array().unwrap())
                    .find(|part| part.get("functionCall").is_some())
                    .unwrap();
                assert_eq!(part["thoughtSignature"], "private-signature");
                assert_eq!(part["functionCall"]["id"], "call_1");
            }
            let (start, end) = streaming::frames(target, tool);
            for frame in start {
                for bytes in frame.chunks(2) {
                    chunk(socket, bytes).unwrap();
                }
            }
            ready.lock().unwrap().recv_timeout(DEADLINE).unwrap();
            for frame in end {
                chunk(socket, &frame).unwrap();
            }
            finish_chunks(socket);
        });
        database
            .add_provider(
                ProviderInput {
                    name: target.as_str().into(),
                    paths: ProviderPaths::single(target),
                    host: "127.0.0.1".into(),
                    port: upstream.address.port(),
                    tls: false,
                    api_key: "stream-key".into(),
                    enabled: true,
                    models_path: "/models".into(),
                    models_protocol: target,
                    anthropic_version: Some("2023-06-01".into()),
                    messages_auth: MessagesAuth::ApiKey,
                    connect_timeout_ms: 2000,
                    read_timeout_ms: 5000,
                    write_timeout_ms: 2000,
                },
                target,
                "upstream-model",
            )
            .await;
        upstreams.push((upstream, requests, release));
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let continuation = Gateway::database(&database.url, MASTER_KEY);
    for (target, (_, requests, release)) in ALL.into_iter().zip(&upstreams) {
        for source in ALL {
            for tool in [false, true] {
                let alias = alias(source, target);
                let mut input = if tool {
                    fixtures::tool_request(source, &alias)
                } else {
                    fixtures::request(source, &alias, "text")
                };
                if source != Protocol::Gemini {
                    input["stream"] = serde_json::json!(true);
                }
                let mut response = gateway.request(
                    "POST",
                    &streaming::path(source, &alias),
                    "Content-Type: application/json\r\nAccept: text/event-stream\r\n",
                    &serde_json::to_vec(&input).unwrap(),
                );
                assert_eq!(
                    response.status,
                    200,
                    "{source:?}->{target:?}: {}",
                    gateway.logs()
                );
                assert_eq!(
                    values(&response.headers, "content-type"),
                    ["text/event-stream"]
                );
                let mut observed = streaming::Observed::new(source);
                while observed.text.is_empty() {
                    let bytes = response.next_chunk().unwrap().expect("结束之前应收到文字");
                    observed.push(source, &bytes);
                }
                assert!("你好".starts_with(&observed.text));
                assert!(
                    observed.decoder.state().ended().is_none(),
                    "文字到达时上游尚未结束"
                );
                release.send(()).unwrap();
                while let Some(bytes) = response.next_chunk().unwrap() {
                    observed.push(source, &bytes);
                }
                observed.finish(source);
                assert_eq!(observed.text, "你好");
                let usage = observed.decoder.state().usage().snapshot().unwrap();
                assert_eq!(usage.input_tokens, Some(10));
                assert_eq!(usage.output_tokens, Some(3));
                assert_eq!(usage.total_tokens, Some(13));
                assert_eq!(usage.cache.read_input_tokens, Some(4));
                let received = requests.recv_timeout(DEADLINE).unwrap();
                assert_eq!(received.target, streaming::path(target, "upstream-model"));
                assert_eq!(values(&received.headers, "accept"), ["text/event-stream"]);
                assert_eq!(
                    fixtures::decode_request(target, &received.body)
                        .generation
                        .stream,
                    target != Protocol::Gemini
                );
                if tool {
                    assert_eq!(observed.tools.len(), 1, "{source:?}->{target:?}");
                    let (id, name, args) = observed.tools.values().next().unwrap();
                    assert_eq!(name, "lookup");
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(args).unwrap(),
                        serde_json::json!({"city":"Paris"})
                    );
                    let call = llmproxy_core::ir::message::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: serde_json::from_str(args).unwrap(),
                    };
                    let mut reply = llmproxy_core::ir::response::Response::new(source);
                    reply
                        .items
                        .push(llmproxy_core::ir::response::Item::ToolCall {
                            call: call.clone(),
                            item_id: None,
                        });
                    let mut next =
                        fixtures::tool_result_request(source, &alias, &input, &reply, &call)
                            .unwrap();
                    if source == Protocol::Gemini {
                        for content in next["contents"].as_array_mut().unwrap() {
                            for part in content["parts"].as_array_mut().unwrap() {
                                if part.get("functionCall").is_some() {
                                    part["thoughtSignature"] =
                                        serde_json::json!("private-signature");
                                }
                            }
                        }
                    }
                    // 新实例从数据库恢复签名；不借助首个实例的内存缓存。
                    release.send(()).unwrap();
                    let response = continuation.request(
                        "POST",
                        &streaming::path(source, &alias),
                        "Content-Type: application/json\r\nAccept: text/event-stream\r\n",
                        &serde_json::to_vec(&next).unwrap(),
                    );
                    assert_eq!(
                        response.status,
                        200,
                        "工具结果 {source:?}->{target:?}: {}",
                        continuation.logs()
                    );
                    let mut followup = streaming::Observed::new(source);
                    followup.push(source, &response.body());
                    followup.finish(source);
                    assert_eq!(followup.text, "你好");
                    assert!(followup.tools.is_empty());
                    requests.recv_timeout(DEADLINE).unwrap();
                }
            }
        }
    }
    let usage_events: usize = [gateway.logs(), continuation.logs()]
        .iter()
        .flat_map(|logs| logs.lines())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|value| value["fields"]["event_kind"] == "usage")
        .count();
    assert_eq!(
        usage_events, 48,
        "每次完整响应只记录一份实际累计计数，包括四个同协议方向"
    );
    streaming::assert_logs(&gateway, 32);
    streaming::assert_logs(&continuation, 16);
}

#[tokio::test]
async fn cross_stream_errors_are_safe_and_never_end_successfully() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (upstream, requests) = Mock::http(move |request, socket| {
            let body = String::from_utf8_lossy(&request.body);
            if body.contains("http-error") {
                support::respond(
                    socket,
                    429,
                    "Content-Type: application/json\r\n",
                    b"private-provider-detail",
                );
                return;
            }
            sse_headers(socket);
            let (start, end) = streaming::frames(target, false);
            for frame in start {
                chunk(socket, &frame).unwrap();
            }
            if body.contains("timeout") {
                std::thread::sleep(std::time::Duration::from_millis(400));
                return;
            }
            if body.contains("transport") {
                for frame in end {
                    let _ = chunk(socket, &frame);
                }
                return;
            }
            if body.contains("truncated") {
                chunk(socket, b"data: {\"private\":").unwrap();
            } else if body.contains("native-error") {
                let error = match target {
                    Protocol::OpenAiChat => {
                        serde_json::json!({"error":{"type":"rate_limit_error","message":"private-provider-detail"}})
                    }
                    Protocol::OpenAiResponses => {
                        serde_json::json!({"type":"error","sequence_number":99,"code":"rate_limit_exceeded","message":"private-provider-detail"})
                    }
                    Protocol::AnthropicMessages => {
                        serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"private-provider-detail"}})
                    }
                    Protocol::Gemini => {
                        serde_json::json!({"error":{"code":429,"message":"private-provider-detail","status":"RESOURCE_EXHAUSTED"}})
                    }
                };
                chunk(socket, format!("data: {error}\n\n").as_bytes()).unwrap();
            } else {
                chunk(socket, b"data: private-provider-detail\n\n").unwrap();
            }
            finish_chunks(socket);
        });
        database
            .add_provider(
                streaming::provider(target, upstream.address.port(), 150),
                target,
                "upstream-model",
            )
            .await;
        upstreams.push((upstream, requests));
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for (target, (_, requests)) in ALL.into_iter().zip(&upstreams) {
        for source in ALL.into_iter().filter(|p| *p != target) {
            for mode in [
                "malformed",
                "native-error",
                "truncated",
                "transport",
                "timeout",
                "http-error",
            ] {
                let alias = alias(source, target);
                let mut body = fixtures::request(source, &alias, mode);
                if source != Protocol::Gemini {
                    body["stream"] = serde_json::json!(true);
                }
                let response = gateway.request(
                    "POST",
                    &streaming::path(source, &alias),
                    "Content-Type: application/json\r\nAccept: text/event-stream\r\n",
                    &serde_json::to_vec(&body).unwrap(),
                );
                assert_eq!(
                    response.status,
                    if mode == "http-error" { 429 } else { 200 },
                    "{source:?}->{target:?} {mode}: {}",
                    gateway.logs()
                );
                let bytes = response.body();
                let text = String::from_utf8_lossy(&bytes);
                assert!(!text.contains("private-provider-detail"));
                if mode != "http-error" {
                    assert!(
                        !text.contains("[DONE]")
                            && !text.contains("\"type\":\"message_stop\"")
                            && !text.contains("\"type\":\"response.completed\"")
                    );
                    let mut framing =
                        llmproxy_core::protocol::stream::sse::Decoder::new(8 * 1024 * 1024);
                    let mut errors = 0;
                    for byte in &bytes {
                        if let Some(frame) = framing.push(*byte).unwrap() {
                            let json: serde_json::Value =
                                serde_json::from_slice(&frame.data).unwrap();
                            if json.get("error").is_some() || json["type"] == "error" {
                                errors += 1;
                                if source == Protocol::OpenAiResponses {
                                    llmproxy_core::protocol::stream::sse::decode(source, &frame)
                                        .unwrap();
                                }
                            }
                        }
                    }
                    assert_eq!(errors, 1, "流内错误只交付一次");
                }
                requests.recv_timeout(DEADLINE).unwrap();
            }
        }
    }
    streaming::assert_logs(&gateway, 72);
}

#[tokio::test]
async fn dropping_streaming_client_cancels_the_upstream() {
    use std::io::Read;
    let database = Database::new().await;
    let (closed, receive) = mpsc::channel();
    let (upstream, _) = Mock::http(move |_, socket| {
        sse_headers(socket);
        let (start, _) = streaming::frames(Protocol::OpenAiChat, false);
        for frame in start {
            chunk(socket, &frame).unwrap();
        }
        socket.set_read_timeout(Some(DEADLINE)).unwrap();
        let mut bytes = [0u8; 1];
        let result = socket.read(&mut bytes);
        closed
            .send(match result {
                Ok(0) => true,
                Err(error) => !matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ),
                _ => false,
            })
            .unwrap();
    });
    database
        .add_provider(
            streaming::provider(Protocol::OpenAiChat, upstream.address.port(), 5000),
            Protocol::OpenAiChat,
            "upstream-model",
        )
        .await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for source in ALL {
        let alias = alias(source, Protocol::OpenAiChat);
        let mut body = fixtures::request(source, &alias, "cancel");
        if source != Protocol::Gemini {
            body["stream"] = serde_json::json!(true);
        }
        let mut response = gateway.request(
            "POST",
            &streaming::path(source, &alias),
            "Content-Type: application/json\r\nAccept: text/event-stream\r\n",
            &serde_json::to_vec(&body).unwrap(),
        );
        assert_eq!(response.status, 200);
        response.next_chunk().unwrap().unwrap();
        let start = std::time::Instant::now();
        drop(response);
        assert!(receive.recv_timeout(DEADLINE).unwrap());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "停止应立即释放上游，不等待读取超时"
        );
    }
    streaming::assert_logs(&gateway, 4);
}
