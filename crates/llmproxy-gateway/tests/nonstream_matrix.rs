mod nonstream;
mod support;

use llmproxy_core::{
    ir::{message::PartKind, response::Item},
    protocol::{MessagesAuth, Protocol},
};
use llmproxy_store::{ProviderInput, ProviderPaths};
use nonstream::{ALL, Database, MASTER_KEY, alias, fixtures, path};
use support::{DEADLINE, Gateway, Mock, respond, values};

#[tokio::test]
async fn four_by_four_nonstream_http_matrix() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (upstream, requests) = Mock::http(move |request, stream| {
            if request.body.windows(6).any(|bytes| bytes == b"reject") {
                respond(
                    stream,
                    429,
                    "Content-Type: application/json\r\n",
                    b"{\"error\":\"private-provider-detail\"}",
                );
            } else {
                let has_result = [
                    "tool_call_id",
                    "tool_result",
                    "function_call_output",
                    "functionResponse",
                ]
                .iter()
                .any(|name| {
                    request
                        .body
                        .windows(name.len())
                        .any(|bytes| bytes == name.as_bytes())
                });
                let tool = !has_result && request.body.windows(4).any(|bytes| bytes == b"tool");
                let mut body = fixtures::response(target, tool);
                if target == Protocol::OpenAiResponses {
                    for status in ["failed", "cancelled", "queued"] {
                        if request
                            .body
                            .windows(status.len())
                            .any(|bytes| bytes == status.as_bytes())
                        {
                            body["status"] = serde_json::json!(status);
                            body["output"] = serde_json::json!([]);
                            if status == "failed" {
                                body["error"] = serde_json::json!({"code":"server_error","message":"private-provider-detail"});
                            }
                        }
                    }
                }
                respond(
                    stream,
                    200,
                    "Content-Type: application/json\r\nETag: upstream-tag\r\n",
                    &serde_json::to_vec(&body).unwrap(),
                );
            }
        });
        database
            .add_provider(
                ProviderInput {
                    name: target.as_str().into(),
                    paths: ProviderPaths::single(target),
                    host: "127.0.0.1".into(),
                    port: upstream.address.port(),
                    tls: false,
                    api_key: "matrix-secret".into(),
                    enabled: true,
                    models_path: "/models".into(),
                    models_protocol: target,
                    anthropic_version: Some("2023-06-01".into()),
                    messages_auth: MessagesAuth::ApiKey,
                    connect_timeout_ms: 1500,
                    read_timeout_ms: 1500,
                    write_timeout_ms: 1500,
                },
                target,
                "upstream-model",
            )
            .await;
        upstreams.push((upstream, requests));
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    // 流式入口不能把 Provider 返回的 JSON 伪装成 SSE，格式错误在客户端发头前拒绝。
    for (target, (upstream, requests)) in ALL.into_iter().zip(&upstreams) {
        for source in ALL.into_iter().filter(|source| *source != target) {
            let alias = alias(source, target);
            let mut body = fixtures::request(source, &alias, "text");
            let path = if source == Protocol::Gemini {
                format!("/v1beta/models/{alias}:streamGenerateContent?alt=sse")
            } else {
                body["stream"] = serde_json::json!(true);
                path(source, &alias)
            };
            let before = upstream.count();
            let response = gateway.request(
                "POST",
                &path,
                "Content-Type: application/json\r\nAccept: text/event-stream\r\n",
                &serde_json::to_vec(&body).unwrap(),
            );
            assert_eq!(
                response.status, 502,
                "流式响应格式 {source:?} -> {target:?}"
            );
            assert_eq!(upstream.count(), before + 1);
            let received = requests.recv_timeout(DEADLINE).unwrap();
            assert_eq!(values(&received.headers, "accept"), ["text/event-stream"]);
        }
    }
    for (target, (_, requests)) in ALL.into_iter().zip(&upstreams) {
        for source in ALL {
            let alias = alias(source, target);
            for prompt in ["text", "tool", "reject"] {
                let response = gateway.request(
                    "POST",
                    &path(source, &alias),
                    "Content-Type: application/json\r\nAuthorization: Bearer client-secret\r\nAccept: text/event-stream\r\n",
                    &serde_json::to_vec(&if prompt == "tool" {
                        fixtures::tool_request(source, &alias)
                    } else {
                        fixtures::request(source, &alias, prompt)
                    })
                    .unwrap(),
                );
                assert_eq!(
                    response.status,
                    if prompt == "reject" { 429 } else { 200 },
                    "{source:?} -> {target:?}, {prompt}: {}",
                    gateway.logs()
                );
                let received = requests.recv_timeout(DEADLINE).unwrap();
                assert_eq!(received.target, path(target, "upstream-model"));
                let ir = fixtures::decode_request(target, &received.body);
                assert!(!ir.generation.stream, "Accept 不应改变正文的非流式意图");
                assert_eq!(ir.generation.max_output_tokens, Some(128));
                if prompt == "tool" {
                    assert_eq!(ir.tools.len(), 1);
                    assert_eq!(ir.tools[0].name, "lookup");
                    assert!(ir.messages[0].parts.iter().any(
                        |part| matches!(&part.kind, PartKind::Text(text) if text.contains("lookup"))
                    ));
                } else {
                    assert!(
                        matches!(&ir.messages[0].parts[0].kind, PartKind::Text(text) if text == prompt)
                    );
                }
                if target != Protocol::Gemini {
                    assert_eq!(ir.model.as_deref(), Some("upstream-model"));
                }
                let (header, expected) = match target {
                    Protocol::AnthropicMessages => ("x-api-key", "matrix-secret"),
                    Protocol::Gemini => ("x-goog-api-key", "matrix-secret"),
                    _ => ("authorization", "Bearer matrix-secret"),
                };
                assert_eq!(values(&received.headers, header), [expected]);
                if source != target {
                    assert!(values(&response.headers, "etag").is_empty());
                    assert_eq!(values(&received.headers, "accept"), ["application/json"]);
                    assert_eq!(values(&received.headers, "accept-encoding"), ["identity"]);
                    assert_eq!(
                        values(&received.headers, "content-length"),
                        [received.body.len().to_string()]
                    );
                }
                let body = response.body();
                if prompt == "reject" {
                    if source != target {
                        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        assert!(error["error"].is_object());
                        assert!(
                            !String::from_utf8_lossy(&body).contains("private-provider-detail")
                        );
                    }
                    continue;
                }
                let ir = fixtures::decode_response(source, &body).unwrap();
                assert_eq!(
                    ir.model.as_deref(),
                    Some(if source == target {
                        "upstream-model"
                    } else {
                        alias.as_str()
                    })
                );
                if prompt == "tool" {
                    let mut calls = ir
                        .messages
                        .iter()
                        .flat_map(|message| &message.parts)
                        .filter_map(|part| match &part.kind {
                            PartKind::ToolCall(call) => Some(call),
                            _ => None,
                        })
                        .chain(ir.items.iter().filter_map(|item| match item {
                            Item::ToolCall { call, .. } => Some(call),
                            _ => None,
                        }));
                    let call = calls.next().expect("工具调用必须保留");
                    assert_eq!(call.name, "lookup");
                    assert_eq!(call.id.as_deref(), Some("call_1"));
                    assert!(calls.next().is_none());
                    let next = fixtures::tool_result_request(
                        source,
                        &alias,
                        &fixtures::tool_request(source, &alias),
                        &ir,
                        call,
                    )
                    .unwrap();
                    let response = gateway.request(
                        "POST",
                        &path(source, &alias),
                        "Content-Type: application/json\r\n",
                        &serde_json::to_vec(&next).unwrap(),
                    );
                    assert_eq!(response.status, 200, "tool result {source:?} -> {target:?}");
                    let request = requests.recv_timeout(DEADLINE).unwrap();
                    let input = fixtures::decode_request(target, &request.body);
                    let results = input
                        .messages
                        .iter()
                        .flat_map(|message| &message.parts)
                        .filter(|part| matches!(part.kind, PartKind::ToolResult(_)))
                        .count()
                        + input
                            .items
                            .iter()
                            .filter(|item| {
                                matches!(item, llmproxy_core::ir::request::Item::ToolResult(_))
                            })
                            .count();
                    assert_eq!(results, 1, "工具结果应在目标请求中只出现一次");
                    let completed = fixtures::decode_response(source, &response.body()).unwrap();
                    assert!(
                        matches!(&completed.messages[0].parts[0].kind, PartKind::Text(text) if text == "OK")
                    );
                } else {
                    assert!(
                        matches!(&ir.messages[0].parts[0].kind, PartKind::Text(text) if text == "OK")
                    );
                }
                let usage = ir.usage.unwrap();
                assert_eq!(usage.input_tokens, Some(10));
                assert_eq!(usage.output_tokens, Some(3));
                assert_eq!(usage.total_tokens, Some(13));
                assert_eq!(usage.cache.read_input_tokens, Some(4));
            }
        }
    }
    // 每个成功正文只记录一次来源用量，工具结果回合也应独立统计。
    let usage_events: Vec<serde_json::Value> = gateway
        .logs()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["fields"]["event_kind"] == "usage")
        .collect();
    assert_eq!(usage_events.len(), 48);
    for event in usage_events {
        assert_eq!(event["fields"]["input_tokens"], 10);
        assert_eq!(event["fields"]["output_tokens"], 3);
        assert_eq!(event["fields"]["cache_read_input_tokens"], 4);
        assert!(event["span"]["request_id"].is_number());
    }
    // Provider 返回 HTTP 200 但生成失败、取消或仍在运行时，跨协议客户端应得到 502。
    // 同协议保留来源状态，避免在网关伪造后台完成结果。
    let (_, requests) = &upstreams[1];
    for source in ALL
        .into_iter()
        .filter(|source| *source != Protocol::OpenAiResponses)
    {
        let alias = alias(source, Protocol::OpenAiResponses);
        for prompt in ["failed", "cancelled", "queued"] {
            let response = gateway.request(
                "POST",
                &path(source, &alias),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::request(source, &alias, prompt)).unwrap(),
            );
            assert_eq!(response.status, 502, "{prompt} -> {source:?}");
            let body = response.body();
            let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(error["error"].is_object());
            assert!(!String::from_utf8_lossy(&body).contains("private-provider-detail"));
            requests.recv_timeout(DEADLINE).unwrap();
        }
    }
    // 请求超出整包转换上限时，在连接 Provider 之前拒绝。
    use std::io::Write;
    let mut stream = gateway.connect();
    let alias = alias(Protocol::Gemini, Protocol::AnthropicMessages);
    write!(stream, "POST {} HTTP/1.1\r\nAuthorization: Bearer {}\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nExpect: 100-continue\r\n\r\n", path(Protocol::Gemini, &alias), gateway.api_key, 8 * 1024 * 1024 + 1).unwrap();
    let response = support::Response::read(stream).unwrap();
    assert_eq!(response.status, 413);
    assert!(upstreams[2].1.try_recv().is_err());
}

#[tokio::test]
async fn gemini_signed_tool_roundtrips_across_instances_and_restart() {
    use llmproxy_core::protocol::OptionalNullable as O;
    let database = Database::new().await;
    let (received, requests) = std::sync::mpsc::channel();
    let upstream = Mock::raw(move |mut stream| {
        // Pingora 可能在正文校验前已连接；本地 422 关闭无正文连接属于预期。
        let Ok(request) = support::Request::read(&mut stream) else {
            return;
        };
        let body: llmproxy_core::protocol::gemini::request::Request =
            serde_json::from_slice(&request.body).unwrap();
        received.send(request).unwrap();
        let result = body
            .contents
            .iter()
            .flat_map(|message| &message.parts)
            .any(|part| part.function_response.as_option().is_some());
        if result {
            let call = body
                .contents
                .iter()
                .flat_map(|message| &message.parts)
                .find(|part| part.function_call.as_option().is_some())
                .unwrap();
            assert_eq!(
                call.thought_signature.as_option().map(String::as_str),
                Some("private-gemini-signature")
            );
            assert!(call.function_call.as_option().unwrap().id.is_missing());
            let result = body
                .contents
                .iter()
                .flat_map(|message| &message.parts)
                .find_map(|part| part.function_response.as_option())
                .unwrap();
            assert!(result.id.is_missing());
        }
        let mut body = fixtures::response(Protocol::Gemini, !result);
        if !result {
            body["candidates"][0]["content"]["parts"][0]["thoughtSignature"] =
                serde_json::json!("private-gemini-signature");
            body["candidates"][0]["content"]["parts"][0]["functionCall"]
                .as_object_mut()
                .unwrap()
                .remove("id");
        }
        respond(
            &mut stream,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&body).unwrap(),
        );
    });
    database
        .add_provider(
            ProviderInput {
                name: "signed-gemini".into(),
                paths: ProviderPaths::single(Protocol::Gemini),
                host: "127.0.0.1".into(),
                port: upstream.address.port(),
                tls: false,
                api_key: "signed-key".into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::Gemini,
                anthropic_version: None,
                messages_auth: MessagesAuth::ApiKey,
                connect_timeout_ms: 1500,
                read_timeout_ms: 1500,
                write_timeout_ms: 1500,
            },
            Protocol::Gemini,
            "upstream-model",
        )
        .await;
    let mut gateway = Gateway::database(&database.url, MASTER_KEY);
    let replica = Gateway::database(&database.url, MASTER_KEY);
    let other_key = database
        .store
        .create_virtual_key(llmproxy_store::VirtualKeyInput {
            name: "Other client".into(),
            all_routes: true,
            model_ids: vec![],
            route_ids: vec![],
            expires_at: None,
        })
        .await
        .unwrap()
        .secret;
    for source in ALL.into_iter().filter(|source| *source != Protocol::Gemini) {
        let alias = alias(source, Protocol::Gemini);
        let initial = fixtures::tool_request(source, &alias);
        let response = gateway.request(
            "POST",
            &path(source, &alias),
            "Content-Type: application/json\r\nAuthorization: Bearer same-client\r\n",
            &serde_json::to_vec(&initial).unwrap(),
        );
        assert_eq!(response.status, 200);
        requests.recv_timeout(DEADLINE).unwrap();
        let body = response.body();
        assert!(!String::from_utf8_lossy(&body).contains("private-gemini-signature"));
        let ir = fixtures::decode_response(source, &body).unwrap();
        let call = ir
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .find_map(|part| match &part.kind {
                PartKind::ToolCall(call) => Some(call),
                _ => None,
            })
            .or_else(|| {
                ir.items.iter().find_map(|item| match item {
                    Item::ToolCall { call, .. } => Some(call),
                    _ => None,
                })
            })
            .unwrap();
        assert!(call.id.as_ref().unwrap().starts_with("call_lp_"));
        let next = fixtures::tool_result_request(source, &alias, &initial, &ir, call).unwrap();
        // 客户端收到调用后重启原实例，副本从共享数据库恢复，不依赖进程内缓存。
        assert!(!gateway.logs().contains("private-gemini-signature"));
        drop(gateway);
        gateway = Gateway::database(&database.url, MASTER_KEY);
        let response = gateway.request_raw(
            "POST",
            &path(source, &alias),
            &format!("Content-Type: application/json\r\nAuthorization: Bearer {other_key}\r\n"),
            &serde_json::to_vec(&next).unwrap(),
        );
        assert_eq!(response.status, 422);
        response.body();
        assert!(requests.try_recv().is_err());
        // 已运行的副本和重启后的原实例都能重复处理原历史。
        for active in [&replica, &gateway] {
            let response = active.request_raw(
                "POST",
                &path(source, &alias),
                &format!(
                    "Content-Type: application/json\r\nx-api-key: {}\r\n",
                    active.api_key
                ),
                &serde_json::to_vec(&next).unwrap(),
            );
            assert_eq!(response.status, 200);
            let received = requests.recv_timeout(DEADLINE).unwrap();
            let native: llmproxy_core::protocol::gemini::request::Request =
                serde_json::from_slice(&received.body).unwrap();
            assert!(
                native
                    .contents
                    .iter()
                    .flat_map(|message| &message.parts)
                    .any(|part| part.thought_signature
                        == O::Value("private-gemini-signature".into()))
            );
            let reply = fixtures::decode_response(source, &response.body()).unwrap();
            assert!(
                matches!(&reply.messages[0].parts[0].kind,PartKind::Text(text) if text == "OK")
            );
        }
        // 数据库中的有效期是唯一依据，其他实例曾处理过也不能复活过期引用。
        let mut admin = toasty::Db::builder().connect(&database.url).await.unwrap();
        toasty::sql::statement("UPDATE tool_continuations SET expires_at = 0")
            .exec(&mut admin)
            .await
            .unwrap();
        let expired = replica.request(
            "POST",
            &path(source, &alias),
            "Content-Type: application/json\r\nAuthorization: Bearer same-client\r\n",
            &serde_json::to_vec(&next).unwrap(),
        );
        assert_eq!(expired.status, 422);
        expired.body();
        assert!(requests.try_recv().is_err());
    }
    // 只破坏本次创建的隔离测试表，模拟 Provider 已生成调用后的存储故障。
    let mut admin = toasty::Db::builder().connect(&database.url).await.unwrap();
    toasty::sql::statement("DROP TABLE tool_continuations")
        .exec(&mut admin)
        .await
        .unwrap();
    let source = Protocol::OpenAiChat;
    let alias = alias(source, Protocol::Gemini);
    let failed = gateway.request(
        "POST",
        &path(source, &alias),
        "Content-Type: application/json\r\n",
        &serde_json::to_vec(&fixtures::tool_request(source, &alias)).unwrap(),
    );
    assert_eq!(failed.status, 503);
    let body = String::from_utf8(failed.body()).unwrap();
    assert!(!body.contains("call_lp_"));
    assert!(!body.contains("private-gemini-signature"));
    requests.recv_timeout(DEADLINE).unwrap();
    assert!(!gateway.logs().contains("private-gemini-signature"));
    assert!(!replica.logs().contains("private-gemini-signature"));
}

#[tokio::test]
async fn buffered_client_cancellation_closes_upstream_before_response_headers() {
    use std::{
        io::{Read, Write},
        net::{Shutdown, TcpStream},
        sync::mpsc,
        time::Duration,
    };
    let database = Database::new().await;
    let (closed, closure) = mpsc::channel();
    let (upstream, requests) = Mock::http(move |_, stream| {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n").unwrap();
        stream.flush().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut byte = [0];
        let cancelled = match stream.read(&mut byte) {
            Ok(0) => true,
            Err(error) => matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ),
            _ => false,
        };
        closed.send(cancelled).unwrap();
    });
    database
        .add_provider(
            ProviderInput {
                name: "cancel".into(),
                paths: ProviderPaths::single(Protocol::AnthropicMessages),
                host: "127.0.0.1".into(),
                port: upstream.address.port(),
                tls: false,
                api_key: "dummy".into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::AnthropicMessages,
                anthropic_version: Some("2023-06-01".into()),
                messages_auth: MessagesAuth::ApiKey,
                connect_timeout_ms: 1500,
                read_timeout_ms: 5000,
                write_timeout_ms: 1500,
            },
            Protocol::AnthropicMessages,
            "upstream-model",
        )
        .await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let alias = alias(Protocol::OpenAiChat, Protocol::AnthropicMessages);
    let body =
        serde_json::to_vec(&fixtures::request(Protocol::OpenAiChat, &alias, "cancel")).unwrap();
    let mut client = TcpStream::connect(gateway.address).unwrap();
    write!(client, "POST /v1/chat/completions HTTP/1.1\r\nAuthorization: Bearer {}\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", gateway.api_key, body.len()).unwrap();
    client.write_all(&body).unwrap();
    client.flush().unwrap();
    requests.recv_timeout(DEADLINE).unwrap();
    client.shutdown(Shutdown::Both).unwrap();
    assert!(
        closure.recv_timeout(DEADLINE).unwrap(),
        "客户端取消后仍在等待 Provider 生成"
    );
}
