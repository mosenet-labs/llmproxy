//! 专项 HTTP 验收：有效媒体、输出 Schema 与缓存写入统计。
mod nonstream;
mod support;

use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::{ProviderInput, ProviderPaths};
use nonstream::{
    ALL, Database, MASTER_KEY, alias,
    fixtures::{self, MediaCase},
    path,
};
use serde_json::{Value, json};
use support::{DEADLINE, Gateway, Mock, respond};

/// 本次测试的 Provider 只有独立模拟地址，不继承真实认证与出口配置。
fn provider(protocol: Protocol, address: std::net::SocketAddr) -> ProviderInput {
    ProviderInput {
        name: format!("capabilities-{}", protocol.as_str()),
        paths: ProviderPaths::single(protocol),
        host: "127.0.0.1".into(),
        port: address.port(),
        tls: false,
        api_key: "dummy-capabilities".into(),
        enabled: true,
        models_path: "/models".into(),
        models_protocol: protocol,
        anthropic_version: (protocol == Protocol::AnthropicMessages).then(|| "2023-06-01".into()),
        messages_auth: MessagesAuth::ApiKey,
        connect_timeout_ms: 1500,
        read_timeout_ms: 1500,
        write_timeout_ms: 1500,
    }
}

/// 直接检查线缆字段，避免用被测 IR 解码器为请求编码器证明正确。
fn native_content(protocol: Protocol, body: &Value) -> &Value {
    match protocol {
        Protocol::OpenAiChat | Protocol::AnthropicMessages => &body["messages"][0]["content"],
        Protocol::OpenAiResponses => &body["input"][0]["content"],
        Protocol::Gemini => &body["contents"][0]["parts"],
    }
}

/// 读取一个原生媒体块的 MIME 与字节，不检查或接受 Provider 私有文件 ID。
fn native_media(protocol: Protocol, body: &Value) -> (String, Vec<u8>) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let part = &native_content(protocol, body)[1];
    let decode = |data: &Value| STANDARD.decode(data.as_str().unwrap()).unwrap();
    let data_url = |value: &Value| {
        let (prefix, data) = value.as_str().unwrap().split_once(";base64,").unwrap();
        (
            prefix.strip_prefix("data:").unwrap().into(),
            STANDARD.decode(data).unwrap(),
        )
    };
    match protocol {
        Protocol::OpenAiChat => match part["type"].as_str().unwrap() {
            "image_url" => data_url(&part["image_url"]["url"]),
            "file" => data_url(&part["file"]["file_data"]),
            "input_audio" => {
                assert_eq!(part["input_audio"]["format"], "wav");
                ("audio/wav".into(), decode(&part["input_audio"]["data"]))
            }
            _ => panic!("无效 Chat 媒体类型"),
        },
        Protocol::OpenAiResponses => match part["type"].as_str().unwrap() {
            "input_image" => data_url(&part["image_url"]),
            "input_file" => data_url(&part["file_data"]),
            _ => panic!("无效 Responses 媒体类型"),
        },
        Protocol::AnthropicMessages => {
            assert_eq!(part["source"]["type"], "base64");
            (
                part["source"]["media_type"].as_str().unwrap().into(),
                decode(&part["source"]["data"]),
            )
        }
        Protocol::Gemini => (
            part["inlineData"]["mimeType"].as_str().unwrap().into(),
            decode(&part["inlineData"]["data"]),
        ),
    }
}

#[tokio::test]
async fn valid_media_and_schema_cross_the_http_boundary() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    let mut receivers = Vec::new();
    for target in ALL {
        let (sender, received) = std::sync::mpsc::channel();
        let upstream = Mock::raw(move |mut stream| {
            // Pingora 可以先连接上游再拒绝正文；只捕获完整到达 Provider 的请求。
            let Ok(request) = support::Request::read(&mut stream) else {
                return;
            };
            sender.send(request).unwrap();
            respond(
                &mut stream,
                200,
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::response(target, false)).unwrap(),
            );
        });
        database
            .add_provider(provider(target, upstream.address), target, "upstream-model")
            .await;
        upstreams.push(upstream);
        receivers.push(received);
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for (target, received) in ALL.into_iter().zip(receivers) {
        for source in ALL {
            let alias = alias(source, target);
            for case in [
                MediaCase::Image,
                MediaCase::Pdf,
                MediaCase::Audio,
                MediaCase::Video,
            ]
            .into_iter()
            .filter(|case| case.supported(source))
            {
                let body = fixtures::media_request(source, &alias, case);
                let response = gateway.request(
                    "POST",
                    &path(source, &alias),
                    "Content-Type: application/json\r\n",
                    &serde_json::to_vec(&body).unwrap(),
                );
                if !case.supported(target) {
                    assert_eq!(response.status, 422, "{source:?} -> {target:?} {case:?}");
                    response.body();
                    assert!(received.try_recv().is_err());
                    continue;
                }
                assert_eq!(response.status, 200, "{source:?} -> {target:?} {case:?}");
                response.body();
                let request = received.recv_timeout(DEADLINE).unwrap();
                let native: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(
                    native_media(target, &native),
                    native_media(source, &body),
                    "{source:?} -> {target:?} {case:?}"
                );
            }
            let schema = fixtures::structured_request(source, &alias);
            let response = gateway.request(
                "POST",
                &path(source, &alias),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&schema).unwrap(),
            );
            assert_eq!(response.status, 200);
            response.body();
            let native: Value =
                serde_json::from_slice(&received.recv_timeout(DEADLINE).unwrap().body).unwrap();
            let output = match target {
                Protocol::OpenAiChat => &native["response_format"]["json_schema"]["schema"],
                Protocol::OpenAiResponses => &native["text"]["format"]["schema"],
                Protocol::AnthropicMessages => &native["output_config"]["format"]["schema"],
                Protocol::Gemini => {
                    assert_eq!(
                        native["generationConfig"]["responseMimeType"],
                        "application/json"
                    );
                    &native["generationConfig"]["responseJsonSchema"]
                }
            };
            assert_eq!(
                output,
                &json!({"type":"object","properties":{"answer":{"type":"string","enum":["OK"]}},"required":["answer"],"additionalProperties":false})
            );
        }
    }
}

#[tokio::test]
async fn numeric_reasoning_budget_is_preserved_or_warned_without_inventing_effort() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    let mut receivers = Vec::new();
    for target in ALL {
        let (sender, received) = std::sync::mpsc::channel();
        let upstream = Mock::raw(move |mut stream| {
            let Ok(request) = support::Request::read(&mut stream) else {
                return;
            };
            sender.send(request).unwrap();
            respond(
                &mut stream,
                200,
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::response(target, false)).unwrap(),
            );
        });
        database
            .add_provider(provider(target, upstream.address), target, "upstream-model")
            .await;
        upstreams.push(upstream);
        receivers.push(received);
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for (target, received) in ALL.into_iter().zip(receivers) {
        for source in [Protocol::AnthropicMessages, Protocol::Gemini] {
            let alias = alias(source, target);
            let response = gateway.request(
                "POST",
                &path(source, &alias),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::reasoning_budget_request(source, &alias)).unwrap(),
            );
            assert_eq!(response.status, 200, "{source:?} -> {target:?}");
            response.body();
            let native: Value =
                serde_json::from_slice(&received.recv_timeout(DEADLINE).unwrap().body).unwrap();
            match target {
                Protocol::OpenAiChat => assert!(native["reasoning_effort"].is_null()),
                Protocol::OpenAiResponses => assert!(native["reasoning"]["effort"].is_null()),
                Protocol::AnthropicMessages => {
                    assert_eq!(native["thinking"]["type"], "enabled");
                    assert_eq!(native["thinking"]["budget_tokens"], 1024);
                    assert_eq!(native["max_tokens"], 2048);
                }
                Protocol::Gemini => {
                    assert_eq!(
                        native["generationConfig"]["thinkingConfig"]["thinkingBudget"],
                        1024
                    );
                    assert!(
                        native["generationConfig"]["thinkingConfig"]["thinkingLevel"].is_null()
                    );
                    assert_eq!(native["generationConfig"]["maxOutputTokens"], 2048);
                }
            }
        }
    }
    // 沿用已确认的有损兼容规则：无法等价换算时告警，不补造努力等级。
    let warnings = gateway
        .logs()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["level"] == "WARN" && event["fields"]["path"] == "reasoning.budget")
        .count();
    assert_eq!(warnings, 4);
}

#[tokio::test]
async fn cache_write_counts_and_ttl_are_preserved_in_telemetry_before_target_projection() {
    let database = Database::new().await;
    let (upstream, received) = Mock::http(|_, stream| {
        let mut response = fixtures::response(Protocol::AnthropicMessages, false);
        response["usage"] = json!({"input_tokens":4,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":3,
            "cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}});
        respond(
            stream,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&response).unwrap(),
        );
    });
    database
        .add_provider(
            provider(Protocol::AnthropicMessages, upstream.address),
            Protocol::AnthropicMessages,
            "upstream-model",
        )
        .await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for source in ALL {
        let alias = alias(source, Protocol::AnthropicMessages);
        let request = fixtures::request(source, &alias, "cache");
        let response = gateway.request(
            "POST",
            &path(source, &alias),
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&request).unwrap(),
        );
        assert_eq!(response.status, 200);
        received.recv_timeout(DEADLINE).unwrap();
        let ir = fixtures::decode_response(source, &response.body()).unwrap();
        let usage = ir.usage.unwrap();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.total_tokens),
            (Some(10), Some(2), Some(12))
        );
        assert_eq!(usage.cache.read_input_tokens, Some(3));
        assert_eq!(
            usage.cache.write_input_tokens,
            if source == Protocol::Gemini {
                None
            } else {
                Some(3)
            }
        );
        if source == Protocol::AnthropicMessages {
            assert_eq!(usage.cache.write_short_input_tokens, Some(1));
            assert_eq!(usage.cache.write_long_input_tokens, Some(2));
        }
    }
    let logs = gateway.logs();
    let usages: Vec<Value> = logs
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|event: &Value| event["fields"]["event_kind"] == "usage")
        .collect();
    assert_eq!(usages.len(), 4);
    for event in usages {
        assert_eq!(event["fields"]["cache_write_input_tokens"], 3);
        assert_eq!(event["fields"]["cache_write_short_input_tokens"], 1);
        assert_eq!(event["fields"]["cache_write_long_input_tokens"], 2);
    }
}

#[tokio::test]
async fn native_execution_outputs_remain_visible_and_do_not_request_client_tools() {
    use llmproxy_core::ir::{message::PartKind, response::Item};
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL.into_iter().filter(|p| *p != Protocol::OpenAiChat) {
        let (upstream, _) = Mock::http(move |_, stream| {
            respond(
                stream,
                200,
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::code_response(target)).unwrap(),
            );
        });
        database
            .add_provider(provider(target, upstream.address), target, "upstream-model")
            .await;
        upstreams.push(upstream);
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for target in ALL.into_iter().filter(|p| *p != Protocol::OpenAiChat) {
        for source in ALL {
            let alias = alias(source, target);
            let response = gateway.request(
                "POST",
                &path(source, &alias),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::request(source, &alias, "calculate")).unwrap(),
            );
            assert_eq!(response.status, 200, "{source:?} -> {target:?}");
            let bytes = response.body();
            let ir = fixtures::decode_response(source, &bytes).unwrap();
            let visible = ir
                .messages
                .iter()
                .flat_map(|m| &m.parts)
                .filter_map(|p| match &p.kind {
                    PartKind::Text(text) => Some(text.as_str()),
                    PartKind::ServerOutput(output) => output.text.as_deref(),
                    _ => None,
                })
                .chain(ir.items.iter().filter_map(|item| match item {
                    Item::ServerOutput(output) => output.text.as_deref(),
                    _ => None,
                }))
                .collect::<String>();
            assert!(visible.contains("1073"), "{source:?} -> {target:?}");
            assert!(
                !ir.messages
                    .iter()
                    .flat_map(|m| &m.parts)
                    .any(|p| matches!(p.kind, PartKind::ToolCall(_)))
            );
            assert!(
                !ir.items
                    .iter()
                    .any(|item| matches!(item, Item::ToolCall { .. }))
            );
            if source != target {
                assert!(!String::from_utf8_lossy(&bytes).contains("private"));
            }
        }
    }
}

#[tokio::test]
async fn output_media_never_succeeds_after_losing_its_native_body() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in [Protocol::Gemini, Protocol::OpenAiChat] {
        let (upstream, _) = Mock::http(move |_, stream| {
            let mut body = fixtures::response(target, false);
            if target == Protocol::Gemini {
                body["candidates"][0]["content"]["parts"] = json!([{"inlineData":{"mimeType":"image/png","data":STANDARD.encode(include_bytes!("nonstream/assets/blue.png"))}}]);
            } else {
                body["choices"][0]["message"]["content"] = Value::Null;
                body["choices"][0]["message"]["audio"] = json!({"id":"audio-private","data":"YQ==","expires_at":100,"transcript":"hello"});
            }
            respond(
                stream,
                200,
                "Content-Type: application/json\r\nETag: private-provider\r\n",
                &serde_json::to_vec(&body).unwrap(),
            );
        });
        database
            .add_provider(provider(target, upstream.address), target, "upstream-model")
            .await;
        upstreams.push(upstream);
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for target in [Protocol::Gemini, Protocol::OpenAiChat] {
        for source in ALL {
            let alias = alias(source, target);
            let response = gateway.request(
                "POST",
                &path(source, &alias),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::request(source, &alias, "generate media")).unwrap(),
            );
            assert_eq!(
                response.status,
                if source == target { 200 } else { 502 },
                "{source:?} -> {target:?}"
            );
            if source != target {
                assert!(!support::values(&response.headers, "etag").contains(&"private-provider"));
            }
            let bytes = response.body();
            if source == target {
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                if target == Protocol::Gemini {
                    assert_eq!(
                        STANDARD
                            .decode(
                                body["candidates"][0]["content"]["parts"][0]["inlineData"]["data"]
                                    .as_str()
                                    .unwrap()
                            )
                            .unwrap(),
                        include_bytes!("nonstream/assets/blue.png")
                    );
                } else {
                    assert_eq!(body["choices"][0]["message"]["audio"]["data"], "YQ==");
                }
            } else {
                assert!(!String::from_utf8_lossy(&bytes).contains("audio-private"));
            }
        }
    }
}
