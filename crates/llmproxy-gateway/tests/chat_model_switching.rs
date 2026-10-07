//! 同一 Console 会话切换协议的 HTTP／浏览器验收，使用隔离数据库与模拟 Provider。
mod nonstream;
#[allow(dead_code)]
mod streaming;
mod support;

use llmproxy_core::{ir::message::PartKind, protocol::Protocol};
use llmproxy_store::{ModelMappingInput, ProviderPaths};
use nonstream::{ALL, Database, MASTER_KEY, fixtures};
use reqwest::Client;
use serde_json::{Value, json};
use std::{net::TcpStream, sync::mpsc::Receiver, time::Duration};
use support::{DEADLINE, Gateway, Mock, Request, chunk, finish_chunks, respond, sse_headers};

struct Fixture {
    gateway: Gateway,
    database: Database,
    upstreams: Vec<(Mock, Receiver<Request>)>,
}

fn answer(protocol: Protocol, request: &Request, socket: &mut TcpStream) {
    if request.method == "GET" {
        // 模型编辑页会查询目录；空目录仍允许回显已保存的模型 ID。
        respond(
            socket,
            200,
            "Content-Type: application/json\r\n",
            br#"{"data":[],"models":[]}"#,
        );
        return;
    }
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let stream = body["stream"].as_bool().unwrap_or(false)
        || request.target.contains(":streamGenerateContent");
    let thought = protocol == Protocol::Gemini
        && body["generationConfig"]["thinkingConfig"]["includeThoughts"] == true;
    if stream {
        sse_headers(socket);
        if body["native_stream_test"] == true {
            chunk(
                socket,
                b"event: provider.native\ndata: {\"opaque\":true}\n\n",
            )
            .unwrap();
        }
        if thought {
            let summary = json!({"responseId":"raw","modelVersion":"upstream-model","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"先检查条件，再计算结果。","thought":true}]}}]});
            chunk(socket, format!("data: {summary}\n\n").as_bytes()).unwrap();
        }
        let (start, end) = streaming::frames(protocol, false);
        for frame in start {
            if chunk(socket, &frame).is_err() {
                return;
            }
        }
        // 留出页面观察生成中控件及停止的时间。
        std::thread::sleep(Duration::from_millis(500));
        for frame in end {
            if chunk(socket, &frame).is_err() {
                return;
            }
        }
        finish_chunks(socket);
    } else {
        let mut response = fixtures::response(protocol, false);
        if thought {
            response["candidates"][0]["content"]["parts"] = json!([
                {"text":"先检查条件，再计算结果。","thought":true},
                {"text":"OK"}
            ]);
        }
        respond(
            socket,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&response).unwrap(),
        );
    }
}

impl Fixture {
    async fn new() -> Self {
        let database = Database::new().await;
        let mut upstreams = Vec::new();
        for protocol in ALL {
            let (mock, received) =
                Mock::http(move |request, socket| answer(protocol, request, socket));
            database
                .add_provider(
                    streaming::provider(protocol, mock.address.port(), 2000),
                    protocol,
                    "upstream-model",
                )
                .await;
            upstreams.push((mock, received));
        }
        // 一个模型支持多个入口，覆盖仅切换协议以及保留当前协议的选择行为。
        let (mock, received) = Mock::http(|request, socket| {
            let protocol = if request.method == "GET" {
                Protocol::OpenAiChat
            } else if request.target.starts_with("/v1beta/") {
                Protocol::Gemini
            } else {
                ALL.into_iter()
                    .find(|p| request.target == p.upstream_path())
                    .unwrap()
            };
            answer(protocol, request, socket);
        });
        let mut provider = streaming::provider(Protocol::OpenAiChat, mock.address.port(), 2000);
        provider.name = "multi-protocol".into();
        provider.paths = ProviderPaths {
            openai_chat: Some(Protocol::OpenAiChat.upstream_path().into()),
            openai_responses: Some(Protocol::OpenAiResponses.upstream_path().into()),
            anthropic_messages: Some(Protocol::AnthropicMessages.upstream_path().into()),
            gemini: Some(Protocol::Gemini.upstream_path().into()),
        };
        let provider = database.store.create(provider).await.unwrap();
        database
            .store
            .create_model(ModelMappingInput {
                thinking: Default::default(),
                alias: "multi-protocol".into(),
                provider_id: provider.id,
                upstream_model_id: "upstream-model".into(),
                protocols: ALL.to_vec(),
                reference_price: None,
            })
            .await
            .unwrap();
        upstreams.push((mock, received));
        // 能力由模型配置声明，客户端入口协议不决定启用参数。
        for model in database.store.list_models().await.unwrap() {
            database
                .store
                .update_model(
                    model.id,
                    model.version,
                    ModelMappingInput {
                        thinking: llmproxy_core::thinking::Config {
                            support: llmproxy_core::thinking::Support::Switchable,
                            enabled: llmproxy_core::ir::request::controls::Reasoning {
                                effort: Some("medium".into()),
                                ..Default::default()
                            },
                        },
                        alias: model.alias,
                        provider_id: model.provider_id,
                        upstream_model_id: model.upstream_model_id,
                        protocols: model.protocols,
                        reference_price: model.reference_price,
                    },
                )
                .await
                .unwrap();
        }
        let model = database
            .store
            .list_models()
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.protocols == vec![Protocol::OpenAiChat])
            .unwrap();
        for (alias, support) in [
            (
                "z-thinking-always",
                llmproxy_core::thinking::Support::AlwaysOn,
            ),
            (
                "z-thinking-unknown",
                llmproxy_core::thinking::Support::Unknown,
            ),
            (
                "z-thinking-unsupported",
                llmproxy_core::thinking::Support::Unsupported,
            ),
        ] {
            database
                .store
                .create_model(ModelMappingInput {
                    thinking: llmproxy_core::thinking::Config {
                        support,
                        enabled: if support == llmproxy_core::thinking::Support::AlwaysOn {
                            model.thinking.enabled.clone()
                        } else {
                            Default::default()
                        },
                    },
                    alias: alias.into(),
                    provider_id: model.provider_id,
                    upstream_model_id: model.upstream_model_id.clone(),
                    protocols: model.protocols.clone(),
                    reference_price: None,
                })
                .await
                .unwrap();
        }
        let gateway = Gateway::database(&database.url, MASTER_KEY);
        Self {
            gateway,
            database,
            upstreams,
        }
    }
    fn base(&self) -> String {
        format!("http://{}/ui", self.gateway.address)
    }
}

async fn procedure(client: &Client, base: &str, name: &str, args: Value) -> Value {
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/{name}"))
        .json(&args)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "{name}: {status} {text}");
    serde_json::from_str(&text).unwrap()
}

fn attribute<'a>(html: &'a str, name: &str) -> &'a str {
    html.split_once(&format!("{name}=\""))
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
}

#[tokio::test]
async fn one_session_switches_four_protocols_in_both_modes_with_all_previous_turns() {
    let fixture = Fixture::new().await;
    let base = fixture.base();
    let client = Client::builder().timeout(DEADLINE).build().unwrap();
    let editor = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = attribute(editor.split_once("name=\"csrf\"").unwrap().1, "value");
    let page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let id = attribute(&page, "data-chat-session");
    let routes = fixture.database.store.list_routes().await.unwrap();
    let mut expected = Vec::new();
    for (turn, stream) in [false, true]
        .into_iter()
        .flat_map(|stream| (0..4).map(move |_| stream))
        .enumerate()
    {
        let source = ALL[turn % 4];
        let target_index = (turn + 1) % 4;
        let target = ALL[target_index];
        let route = routes
            .iter()
            .find(|route| route.protocol == source && route.provider_protocol == target)
            .unwrap();
        let model = format!("route:{}", route.id);
        assert_eq!(
            procedure(
                &client,
                &base,
                "switch-chat",
                json!([csrf, id, model, source.as_str()])
            )
            .await,
            ""
        );
        let prompt = format!("question-{turn}");
        assert_eq!(
            procedure(&client, &base, "begin-chat", json!([csrf, id, prompt])).await,
            true
        );
        // 生成意图建立后，即使还未发送 HTTP，服务端也不允许选择竞争。
        assert!(
            procedure(
                &client,
                &base,
                "switch-chat",
                json!([csrf, id, model, source.as_str()])
            )
            .await
            .as_str()
            .unwrap()
            .contains("正在生成")
        );
        assert_eq!(
            procedure(&client, &base, "send-chat", json!([csrf, id, stream])).await,
            true
        );
        expected.push(prompt);
        let captured = fixture.upstreams[target_index]
            .1
            .recv_timeout(DEADLINE)
            .unwrap();
        let decoded = fixtures::decode_request(target, &captured.body);
        let texts: Vec<_> = decoded
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .filter_map(|part| match &part.kind {
                PartKind::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, expected, "turn {turn}: {source:?} → {target:?}");
        expected.push(if stream { "你好" } else { "OK" }.into());
        assert!(captured.target.starts_with(if target == Protocol::Gemini {
            "/v1beta/models/upstream-model:"
        } else {
            target.upstream_path()
        }));
        let invalid = procedure(
            &client,
            &base,
            "switch-chat",
            json!([csrf, id, "missing", "gemini"]),
        )
        .await;
        assert!(!invalid.as_str().unwrap().is_empty());
    }
    // 停止回合不会将生成失败文案作为下一轮 assistant 输入。
    let multi = fixture
        .database
        .store
        .list_models()
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.alias == "multi-protocol")
        .unwrap()
        .id
        .to_string();
    assert_eq!(
        procedure(
            &client,
            &base,
            "default-chat-protocol",
            json!([csrf, multi, "gemini"])
        )
        .await,
        "gemini"
    );
    for protocol in ALL {
        assert_eq!(
            procedure(
                &client,
                &base,
                "switch-chat",
                json!([csrf, id, multi, protocol.as_str()])
            )
            .await,
            ""
        );
        let prompt = format!("only-protocol-{}", protocol.as_str());
        assert_eq!(
            procedure(&client, &base, "begin-chat", json!([csrf, id, prompt])).await,
            true
        );
        assert_eq!(
            procedure(&client, &base, "send-chat", json!([csrf, id, false])).await,
            true
        );
        expected.push(prompt);
        let captured = fixture.upstreams[4].1.recv_timeout(DEADLINE).unwrap();
        let decoded = fixtures::decode_request(protocol, &captured.body);
        let texts: Vec<_> = decoded
            .messages
            .iter()
            .flat_map(|m| &m.parts)
            .filter_map(|p| match &p.kind {
                PartKind::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, expected);
        expected.push("OK".into());
    }
    assert_eq!(
        procedure(&client, &base, "begin-chat", json!([csrf, id, "stopped"])).await,
        true
    );
    assert_eq!(
        procedure(&client, &base, "stop-chat", json!([csrf, id])).await,
        true
    );
    assert_eq!(
        procedure(&client, &base, "send-chat", json!([csrf, id, true])).await,
        true
    );
    let new = procedure(&client, &base, "new-chat", json!([csrf, id])).await;
    assert_ne!(new.as_str().unwrap(), id);
    assert_eq!(
        procedure(&client, &base, "begin-chat", json!([csrf, new, "fresh"])).await,
        true
    );
    assert_eq!(
        procedure(&client, &base, "send-chat", json!([csrf, new, false])).await,
        true
    );
    let captured = fixture.upstreams[4].1.recv_timeout(DEADLINE).unwrap();
    let decoded = fixtures::decode_request(Protocol::Gemini, &captured.body);
    assert_eq!(decoded.messages.len(), 1);
    assert_eq!(
        decoded.messages[0].parts[0].kind,
        PartKind::Text("fresh".into())
    );
}

#[tokio::test]
#[ignore = "手动浏览器验收：隔离数据库与模拟 Provider，最多等待 15 分钟"]
async fn browser_model_switching_fixture() {
    let fixture = Fixture::new().await;
    println!("MODEL_SWITCHING_BROWSER_URL={}/chat", fixture.base());
    tokio::time::sleep(Duration::from_secs(15 * 60)).await;
}

/// HTTP 验收逐方向独立检查 Provider 字段，避免仅通过同一 Codec 自测。
#[tokio::test]
async fn thinking_switches_all_sixteen_directions_and_keeps_native_streaming() {
    use llmproxy_core::thinking::{Choice, HEADER};
    let fixture = Fixture::new().await;
    let client = Client::builder().timeout(DEADLINE).build().unwrap();
    for (index, target) in ALL.into_iter().enumerate() {
        for source in ALL {
            for choice in [Choice::Enabled, Choice::Disabled] {
                let alias = nonstream::alias(source, target);
                let mut body = fixtures::request(source, &alias, "hi");
                match source {
                    Protocol::OpenAiChat => body["reasoning_effort"] = json!("high"),
                    Protocol::OpenAiResponses => body["reasoning"] = json!({"effort":"high"}),
                    Protocol::AnthropicMessages => {
                        body["thinking"] = json!({"type":"enabled","budget_tokens":1024});
                        body["max_tokens"] = json!(2048);
                        body["output_config"] = json!({"effort":"high"});
                    }
                    Protocol::Gemini => {
                        body["generationConfig"]["thinkingConfig"] = json!({"thinkingLevel":"high"})
                    }
                }
                let response = client
                    .post(format!(
                        "http://{}{}",
                        fixture.gateway.address,
                        nonstream::path(source, &alias)
                    ))
                    .header(HEADER, choice.as_str())
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert!(
                    response.status().is_success(),
                    "{source:?} → {target:?}: {}",
                    response.text().await.unwrap()
                );
                let request = fixture.upstreams[index].1.recv_timeout(DEADLINE).unwrap();
                assert!(
                    !request
                        .headers
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(HEADER))
                );
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let enabled = choice == Choice::Enabled;
                match target {
                    Protocol::OpenAiChat => assert_eq!(
                        body["reasoning_effort"],
                        if enabled { "medium" } else { "none" }
                    ),
                    Protocol::OpenAiResponses => {
                        assert_eq!(
                            body["reasoning"]["effort"],
                            if enabled { "medium" } else { "none" }
                        );
                        if enabled {
                            assert_eq!(body["reasoning"]["summary"], "auto");
                        }
                    }
                    Protocol::AnthropicMessages => {
                        assert_eq!(
                            body["thinking"]["type"],
                            if enabled { "adaptive" } else { "disabled" }
                        );
                        if !enabled {
                            assert!(body["output_config"]["effort"].is_null());
                            assert!(body["thinking"]["budget_tokens"].is_null());
                        }
                    }
                    Protocol::Gemini => {
                        if enabled {
                            assert_eq!(
                                body["generationConfig"]["thinkingConfig"]["includeThoughts"],
                                true
                            );
                            assert_eq!(
                                body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                                "medium"
                            );
                        } else {
                            assert_eq!(
                                body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
                                0
                            );
                            assert!(
                                body["generationConfig"]["thinkingConfig"]["thinkingLevel"]
                                    .is_null()
                            );
                        }
                    }
                }
            }
        }
        // 同协议启用设置仍在 Provider 完成前交付首块，原生扩展事件逐字节保留。
        let alias = nonstream::alias(target, target);
        let mut body = fixtures::request(target, &alias, "native stream");
        body["native_stream_test"] = json!(true);
        let path = if target == Protocol::Gemini {
            format!("/v1beta/models/{alias}:streamGenerateContent?alt=sse")
        } else {
            body["stream"] = json!(true);
            target.upstream_path().into()
        };
        let mut response = client
            .post(format!("http://{}{}", fixture.gateway.address, path))
            .header(HEADER, "enabled")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let first = tokio::time::timeout(Duration::from_millis(350), response.chunk())
            .await
            .expect("同协议流式首块应在 Provider 结束前到达")
            .unwrap()
            .unwrap();
        let mut received = first.to_vec();
        while let Some(chunk) = response.chunk().await.unwrap() {
            received.extend_from_slice(&chunk);
        }
        assert!(
            String::from_utf8(received)
                .unwrap()
                .contains("event: provider.native\ndata: {\"opaque\":true}")
        );
        let request = fixture.upstreams[index].1.recv_timeout(DEADLINE).unwrap();
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(HEADER))
        );
    }
    for (alias, choice) in [
        ("z-thinking-always", "disabled"),
        ("z-thinking-unknown", "enabled"),
        ("z-thinking-unsupported", "enabled"),
    ] {
        let response = client
            .post(format!(
                "http://{}/v1/chat/completions",
                fixture.gateway.address
            ))
            .header(HEADER, choice)
            .json(&fixtures::request(Protocol::OpenAiChat, alias, "rejected"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 422);
        assert!(response.text().await.unwrap().contains("思考"));
    }
    for headers in [vec!["invalid"], vec!["enabled", "disabled"]] {
        let mut builder = client.post(format!(
            "http://{}/v1/chat/completions",
            fixture.gateway.address
        ));
        for value in headers {
            builder = builder.header(HEADER, value);
        }
        let response = builder
            .json(&fixtures::request(
                Protocol::OpenAiChat,
                "z-thinking-always",
                "rejected",
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
    }
    assert!(
        fixture.upstreams[0].1.try_recv().is_err(),
        "拒绝请求不能发送到 Provider"
    );
}

/// 请求开关经实际目标写入 includeThoughts，四种客户端都应收到摘要且不重复正文。
#[tokio::test]
async fn enabled_gemini_summaries_reach_all_clients_in_both_modes() {
    use llmproxy_core::{
        adapter::protocol_codec::ProtocolCodec,
        ir::response::{Item, Status},
        protocol::wire,
        thinking::HEADER,
    };
    let fixture = Fixture::new().await;
    let client = Client::builder().timeout(DEADLINE).build().unwrap();
    let summary = "先检查条件，再计算结果。";
    for source in ALL {
        for stream in [false, true] {
            let alias = nonstream::alias(source, Protocol::Gemini);
            let mut body = fixtures::request(source, &alias, "summary");
            body["stream"] = json!(stream);
            let path = if stream {
                streaming::path(source, &alias)
            } else {
                nonstream::path(source, &alias)
            };
            let response = client
                .post(format!("http://{}{}", fixture.gateway.address, path))
                .header(HEADER, "enabled")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "{source:?}, stream={stream}"
            );
            let bytes = response.bytes().await.unwrap();
            let text = if stream {
                let mut observed = streaming::Observed::new(source);
                observed.push(source, &bytes);
                observed.finish(source);
                assert_eq!(observed.decoder.state().ended(), Some(Status::Completed));
                observed.text
            } else {
                let raw = wire::decode_response(source, &bytes).unwrap();
                let ir = source.decode_response(&raw).unwrap();
                let mut text = String::new();
                for message in &ir.messages {
                    for part in &message.parts {
                        match &part.kind {
                            PartKind::Text(value) => text.push_str(value),
                            PartKind::Reasoning(value) => text.push_str(value.as_str().unwrap()),
                            _ => {}
                        }
                    }
                }
                for item in &ir.items {
                    if let Item::Reasoning(value) = item {
                        text.push_str(value);
                    }
                }
                text
            };
            assert_eq!(
                text.matches(summary).count(),
                1,
                "{source:?}, stream={stream}: {text}"
            );
            assert!(text.contains(if stream { "你好" } else { "OK" }));
            let captured = fixture.upstreams[3].1.recv_timeout(DEADLINE).unwrap();
            let body: Value = serde_json::from_slice(&captured.body).unwrap();
            assert_eq!(
                body["generationConfig"]["thinkingConfig"]["includeThoughts"],
                true
            );
        }
    }
}
