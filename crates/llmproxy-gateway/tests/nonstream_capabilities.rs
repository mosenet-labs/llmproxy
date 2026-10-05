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
