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

fn answer(protocol: Protocol, request: &Request, socket: &mut TcpStream, pause: Duration) {
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
        let (mut start, end) = streaming::frames(protocol, false);
        if protocol == Protocol::AnthropicMessages {
            start[0]=b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"raw\",\"type\":\"message\",\"model\":\"upstream-model\",\"role\":\"assistant\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":6,\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":2,\"cache_creation\":{\"ephemeral_5m_input_tokens\":1,\"ephemeral_1h_input_tokens\":1},\"output_tokens\":0}}}\n\n".to_vec();
        }
        for frame in start {
            if chunk(socket, &frame).is_err() {
                return;
            }
        }
        // 留出页面观察生成中控件及停止的时间。
        std::thread::sleep(pause);
        for frame in end {
            if chunk(socket, &frame).is_err() {
                return;
            }
        }
        finish_chunks(socket);
    } else {
        let mut response = fixtures::response(protocol, false);
        if protocol == Protocol::AnthropicMessages {
            response["usage"]["cache_creation_input_tokens"] = json!(2);
            response["usage"]["cache_creation"] =
                json!({"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":1});
        }
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
        Self::with_pause(Duration::from_millis(500)).await
    }
    async fn with_pause(pause: Duration) -> Self {
        let database = Database::new().await;
        let mut upstreams = Vec::new();
        for protocol in ALL {
            let (mock, received) =
                Mock::http(move |request, socket| answer(protocol, request, socket, pause));
            database
                .add_provider(
                    streaming::provider(
                        protocol,
                        mock.address.port(),
                        2000.max(pause.as_millis() as u64 + 1000),
                    ),
                    protocol,
                    "upstream-model",
                )
                .await;
            upstreams.push((mock, received));
        }
        // 一个模型支持多个入口，覆盖仅切换协议以及保留当前协议的选择行为。
        let (mock, received) = Mock::http(move |request, socket| {
            let protocol = if request.method == "GET" {
                Protocol::OpenAiChat
            } else if request.target.starts_with("/v1beta/") {
                Protocol::Gemini
            } else {
                ALL.into_iter()
                    .find(|p| request.target == p.upstream_path())
                    .unwrap()
            };
            answer(protocol, request, socket, pause);
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

/// 等待 Gateway 异步收尾，统计与正文两个保存事务可以先后完成。
async fn saved_turn(
    fixture: &Fixture,
    id: &str,
    sequence: usize,
) -> llmproxy_store::chat_history::Turn {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let turns = fixture.database.store.chat_turns(id).await.unwrap();
        if let Some(turn) = turns.get(sequence - 1)
            && turn.call_finished
            && turn.status != llmproxy_store::chat_history::Status::Generating
        {
            return turn.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "来源统计未保存");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 四同协议与十二跨方向在两种模式都从来源记录统计，目标编码不能丢失缓存写入。
#[tokio::test]
async fn history_records_source_usage_in_all_directions_and_survives_restart() {
    let mut fixture = Fixture::new().await;
    let client = Client::builder().timeout(DEADLINE).build().unwrap();
    let mut base = fixture.base();
    let editor = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let mut csrf = attribute(editor.split_once("name=\"csrf\"").unwrap().1, "value").to_owned();
    let page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let id = attribute(&page, "data-chat-session").to_owned();
    let routes = fixture.database.store.list_routes().await.unwrap();
    let mut sequence = 0;
    for stream in [false, true] {
        for source in ALL {
            for (target_index, target) in ALL.into_iter().enumerate() {
                let route = routes
                    .iter()
                    .find(|r| r.protocol == source && r.provider_protocol == target)
                    .unwrap();
                assert_eq!(
                    procedure(
                        &client,
                        &base,
                        "switch-chat",
                        json!([csrf, id, format!("route:{}", route.id), source.as_str()])
                    )
                    .await,
                    ""
                );
                assert_eq!(
                    procedure(
                        &client,
                        &base,
                        "begin-chat",
                        json!([csrf, id, format!("history-{sequence}")])
                    )
                    .await,
                    true
                );
                assert_eq!(
                    procedure(&client, &base, "send-chat", json!([csrf, id, stream])).await,
                    true
                );
                sequence += 1;
                let turn = saved_turn(&fixture, &id, sequence).await;
                assert_eq!(
                    turn.status,
                    llmproxy_store::chat_history::Status::Completed,
                    "{source:?}->{target:?} stream={stream}"
                );
                assert_eq!(
                    turn.usage_state,
                    llmproxy_store::chat_history::UsageState::Final
                );
                let a = turn.actual.unwrap();
                assert_eq!(a.protocol, target);
                assert_eq!(a.provider_name, target.as_str());
                assert_eq!(a.upstream_model, "upstream-model");
                assert_eq!(a.reported_model.as_deref(), Some("upstream-model"));
                let usage = turn.usage.unwrap();
                assert_eq!(
                    usage.input_tokens,
                    Some(if target == Protocol::AnthropicMessages {
                        12
                    } else {
                        10
                    })
                );
                if target == Protocol::AnthropicMessages {
                    assert_eq!(usage.cache.write_input_tokens, Some(2));
                    assert_eq!(usage.cache.write_short_input_tokens, Some(1));
                    assert_eq!(usage.cache.write_long_input_tokens, Some(1));
                }
                assert_eq!(usage.output_tokens, Some(3));
                assert_eq!(usage.cache.read_input_tokens, Some(4));
                let request = fixture.upstreams[target_index]
                    .1
                    .recv_timeout(DEADLINE)
                    .unwrap();
                assert!(!request.headers.iter().any(|(key, _)| {
                    key.eq_ignore_ascii_case(llmproxy_store::chat_history::AUTH_HEADER)
                }));
                assert!(!request.headers.iter().any(|(key, _)| {
                    key.eq_ignore_ascii_case(llmproxy_store::chat_history::REQUEST_HEADER)
                }));
            }
        }
    }
    let turns = fixture.database.store.chat_turns(&id).await.unwrap();
    let sum = llmproxy_store::chat_history::Summary::from_turns(&turns);
    assert_eq!(sum.totals.input.total, Some(336));
    assert_eq!(sum.totals.output.total, Some(96));
    assert_eq!(sum.models.len(), 4);
    assert!((sum.totals.hit_rate().unwrap() - 128.0 / 336.0 * 100.0).abs() < 0.001);
    // 替换进程时旧 Gateway 已退出；新 Console 从同一数据库恢复 IR。
    let Fixture {
        gateway,
        database,
        upstreams,
    } = fixture;
    drop(gateway);
    fixture = Fixture {
        gateway: Gateway::database(&database.url, MASTER_KEY),
        database,
        upstreams,
    };
    base = fixture.base();
    let page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(attribute(&page, "data-chat-session"), id);
    assert!(page.contains("history-0"));
    assert!(page.contains("会话累计用量"));
    assert!(page.contains("输入 336"));
    let editor = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    csrf = attribute(editor.split_once("name=\"csrf\"").unwrap().1, "value").into();
    assert_eq!(
        procedure(
            &client,
            &base,
            "begin-chat",
            json!([csrf, id, "after-restart"])
        )
        .await,
        true
    );
    assert_eq!(
        procedure(&client, &base, "send-chat", json!([csrf, id, false])).await,
        true
    );
    let captured = fixture.upstreams[3].1.recv_timeout(DEADLINE).unwrap();
    let decoded = fixtures::decode_request(Protocol::Gemini, &captured.body);
    assert_eq!(decoded.messages.len(), 65);
    saved_turn(&fixture, &id, 33).await;
    // 归档允许查看历史，但不能绕过页面继续发送或开始新轮次。
    procedure(
        &client,
        &base,
        "change-chat-history",
        json!([csrf, "", id, "archive"]),
    )
    .await;
    assert_eq!(
        procedure(&client, &base, "open-chat", json!([csrf, id])).await,
        ""
    );
    assert_eq!(
        procedure(&client, &base, "begin-chat", json!([csrf, id, "archived"])).await,
        false
    );
    assert_eq!(
        procedure(&client, &base, "send-chat", json!([csrf, id, true])).await,
        false
    );
    assert_eq!(
        fixture.database.store.chat_turns(&id).await.unwrap().len(),
        33
    );
    procedure(
        &client,
        &base,
        "change-chat-history",
        json!([csrf, "", id, "restore"]),
    )
    .await;
    // 删除配置后历史不受级联影响；没有可用模型时仍可浏览和删除会话。
    for route in fixture.database.store.list_routes().await.unwrap() {
        fixture
            .database
            .store
            .delete_route(route.id, route.version)
            .await
            .unwrap();
    }
    for model in fixture.database.store.list_models().await.unwrap() {
        fixture
            .database
            .store
            .delete_model(model.id, model.version)
            .await
            .unwrap();
    }
    for provider in fixture.database.store.list().await.unwrap() {
        fixture
            .database
            .store
            .set_enabled(provider.id, provider.version, false)
            .await
            .unwrap();
        let current = fixture.database.store.get(provider.id).await.unwrap();
        fixture
            .database
            .store
            .delete(current.id, current.version)
            .await
            .unwrap();
    }
    let page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("暂无可聊天的模型"));
    assert!(page.contains("history-0"));
    assert!(page.contains("upstream-model"));
    assert!(page.contains("模型配置已失效"));
    // 行菜单使用统一入口；配置失效后仍可归档、恢复和删除历史。
    let archived = procedure(
        &client,
        &base,
        "change-chat-history",
        json!([csrf, id, id, "archive"]),
    )
    .await;
    let replacement = archived["ok"]["v"][0].as_str().unwrap().to_owned();
    assert_ne!(replacement, id);
    assert!(
        fixture
            .database
            .store
            .get_chat_conversation(&id)
            .await
            .unwrap()
            .archived
    );
    assert_eq!(
        fixture.database.store.chat_turns(&id).await.unwrap().len(),
        33
    );
    let restored = procedure(
        &client,
        &base,
        "change-chat-history",
        json!([csrf, replacement, id, "restore"]),
    )
    .await;
    assert!(restored["ok"]["v"].is_null());
    assert!(
        !fixture
            .database
            .store
            .get_chat_conversation(&id)
            .await
            .unwrap()
            .archived
    );
    assert_eq!(
        procedure(&client, &base, "open-chat", json!([csrf, id])).await,
        ""
    );
    let next = procedure(
        &client,
        &base,
        "change-chat-history",
        json!([csrf, id, id, "delete"]),
    )
    .await;
    assert_eq!(next["ok"]["v"][0].as_str().unwrap(), replacement);
    assert!(
        fixture
            .database
            .store
            .get_chat_conversation(&id)
            .await
            .is_err()
    );
}

/// 停止跨协议 SSE 保留来源已报告消耗；伪造专用头不能认领轮次。
#[tokio::test]
async fn history_keeps_partial_usage_on_stop_and_rejects_forged_association() {
    let mut fixture = Fixture::with_pause(Duration::from_secs(2)).await;
    let client = Client::builder().timeout(DEADLINE).build().unwrap();
    let base = fixture.base();
    let editor = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = attribute(editor.split_once("name=\"csrf\"").unwrap().1, "value").to_owned();
    let page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let id = attribute(&page, "data-chat-session").to_owned();
    let routes = fixture.database.store.list_routes().await.unwrap();
    let route = routes
        .iter()
        .find(|r| {
            r.protocol == Protocol::OpenAiChat && r.provider_protocol == Protocol::AnthropicMessages
        })
        .unwrap();
    assert_eq!(
        procedure(
            &client,
            &base,
            "switch-chat",
            json!([csrf, id, format!("route:{}", route.id), "openai_chat"])
        )
        .await,
        ""
    );
    assert_eq!(
        procedure(&client, &base, "begin-chat", json!([csrf, id, "stop-test"])).await,
        true
    );
    let record = fixture
        .database
        .store
        .chat_turns(&id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let response = client
        .post(format!(
            "http://{}{}",
            fixture.gateway.address,
            Protocol::OpenAiChat.upstream_path()
        ))
        .header(llmproxy_store::chat_history::AUTH_HEADER, "forged")
        .header(llmproxy_store::chat_history::REQUEST_HEADER, &record.id)
        .json(&fixtures::request(
            Protocol::OpenAiChat,
            &route.name,
            "forged",
        ))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    response.bytes().await.unwrap();
    fixture.upstreams[2].1.recv_timeout(DEADLINE).unwrap();
    assert!(
        !fixture
            .database
            .store
            .chat_turn(&record.id)
            .await
            .unwrap()
            .call_started
    );
    let send_client = client.clone();
    let send_base = base.clone();
    let send_args = json!([csrf, id, true]);
    let sending =
        tokio::spawn(
            async move { procedure(&send_client, &send_base, "send-chat", send_args).await },
        );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if fixture.upstreams[2].1.try_recv().is_ok() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    let active_page = tokio::time::timeout(Duration::from_millis(500), async {
        client
            .get(format!("{base}/chat"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await
    .expect("生成中的 HTTP 首屏不得等待整轮结束");
    assert!(active_page.contains("停止生成"));
    assert!(active_page.contains("你好"));
    assert_eq!(
        procedure(&client, &base, "stop-chat", json!([csrf, id])).await,
        true
    );
    assert_eq!(sending.await.unwrap(), true);
    let turn = saved_turn(&fixture, &id, 1).await;
    assert_eq!(turn.status, llmproxy_store::chat_history::Status::Cancelled);
    assert_eq!(
        turn.usage_state,
        llmproxy_store::chat_history::UsageState::Partial
    );
    assert_eq!(turn.usage.as_ref().unwrap().input_tokens, Some(12));
    assert_eq!(turn.usage.as_ref().unwrap().output_tokens, Some(0));
    assert_eq!(turn.reply.as_ref().unwrap().content, "你好");
    assert!(turn.reply.as_ref().unwrap().history.items.is_empty());
    assert_eq!(
        procedure(&client, &base, "begin-chat", json!([csrf, id, "abandoned"])).await,
        true
    );
    let Fixture {
        gateway,
        database,
        upstreams,
    } = fixture;
    drop(gateway);
    fixture = Fixture {
        gateway: Gateway::database(&database.url, MASTER_KEY),
        database,
        upstreams,
    };
    let records = fixture.database.store.chat_turns(&id).await.unwrap();
    assert_eq!(
        records[1].status,
        llmproxy_store::chat_history::Status::Interrupted
    );
    assert!(records[1].usage.is_none());
    assert_eq!(records[0].usage.as_ref().unwrap().input_tokens, Some(12));
    let page = client
        .get(format!("{}/chat", fixture.base()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("半") || page.contains("你好"));
    assert!(page.contains("部分用量"));
    assert!(page.contains("服务重启，生成已中断"));
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
        saved_turn(&fixture, id, turn + 1).await;
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
    for (offset, protocol) in ALL.into_iter().enumerate() {
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
        saved_turn(&fixture, id, 9 + offset).await;
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
        false
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
async fn history_generates_multiple_conversations_without_holding_http_requests() {
    use llmproxy_store::chat_history::Status;
    let fixture = Fixture::with_pause(Duration::from_secs(3)).await;
    let client = Client::builder().timeout(DEADLINE).build().unwrap();
    let base = fixture.base();
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
    let first = attribute(&page, "data-chat-session").to_owned();
    let source = fixture
        .database
        .store
        .get_chat_conversation(&first)
        .await
        .unwrap()
        .selection
        .protocol;
    let index = ALL.iter().position(|protocol| *protocol == source).unwrap();
    assert_eq!(
        procedure(
            &client,
            &base,
            "begin-chat",
            json!([csrf, first, "conversation-a"])
        )
        .await,
        true
    );
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(500),
            procedure(&client, &base, "send-chat", json!([csrf, first, true]))
        )
        .await
        .expect("提交不能等待流式回复完成"),
        true
    );
    // 当前会话生成中也能新建，并保持两个会话各自的生成互斥。
    let second = procedure(&client, &base, "new-chat", json!([csrf, first]))
        .await
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        procedure(
            &client,
            &base,
            "begin-chat",
            json!([csrf, second, "conversation-b"])
        )
        .await,
        true
    );
    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(500),
            procedure(&client, &base, "send-chat", json!([csrf, second, true]))
        )
        .await
        .expect("第二个会话应独立提交"),
        true
    );
    for id in [&first, &second] {
        assert_eq!(
            fixture.database.store.chat_turns(id).await.unwrap()[0].status,
            Status::Generating
        );
        assert_eq!(
            procedure(&client, &base, "open-chat", json!([csrf, id])).await,
            ""
        );
        assert_eq!(
            procedure(&client, &base, "begin-chat", json!([csrf, id, "duplicate"])).await,
            false
        );
        assert_eq!(
            procedure(&client, &base, "send-chat", json!([csrf, id, true])).await,
            false
        );
        assert!(
            fixture
                .database
                .store
                .set_chat_archived(id, true)
                .await
                .is_err()
        );
        assert!(
            fixture
                .database
                .store
                .delete_chat_conversation(id)
                .await
                .is_err()
        );
    }
    // HTTP 页面刷新只读取快照，不重发 Provider 请求，也不等待两个后台任务结束。
    let active = tokio::time::timeout(Duration::from_millis(500), async {
        client
            .get(format!("{base}/chat"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert!(active.contains("停止生成"));
    assert!(active.contains("conversation-a"));
    assert!(active.contains("conversation-b"));
    let mut prompts = Vec::new();
    for _ in 0..2 {
        let request = fixture.upstreams[index].1.recv_timeout(DEADLINE).unwrap();
        let decoded = fixtures::decode_request(source, &request.body);
        assert_eq!(decoded.messages.len(), 1, "两会话不能混用历史");
        prompts.push(decoded.messages[0].parts[0].kind.clone());
    }
    assert_eq!(
        procedure(&client, &base, "stop-chat", json!([csrf, first])).await,
        true
    );
    let cancelled = saved_turn(&fixture, &first, 1).await;
    assert_eq!(cancelled.status, Status::Cancelled);
    assert_eq!(
        fixture.database.store.chat_turns(&second).await.unwrap()[0].status,
        Status::Generating
    );
    // 更新时间以秒记录，固定最高 ID 让同秒的空闲首屏排序也保持确定。
    let idle_id = "ffffffffffffffffffffffffffffffff";
    let selection = fixture
        .database
        .store
        .get_chat_conversation(&first)
        .await
        .unwrap()
        .selection;
    fixture
        .database
        .store
        .create_chat_conversation(idle_id, &selection)
        .await
        .unwrap();
    let idle_page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(attribute(&idle_page, "data-chat-session"), idle_id);
    assert_eq!(
        attribute(&idle_page, "data-chat-resume"),
        "true",
        "空闲首屏也要恢复后台会话的订阅"
    );
    // 提交请求已经结束，第二个会话仍在无人订阅时完成保存及最终来源用量。
    let completed = saved_turn(&fixture, &second, 1).await;
    assert_eq!(completed.status, Status::Completed);
    assert_eq!(completed.reply.as_ref().unwrap().content, "你好");
    assert_eq!(completed.usage.as_ref().unwrap().output_tokens, Some(3));
    assert!(prompts.contains(&PartKind::Text("conversation-a".into())));
    assert!(prompts.contains(&PartKind::Text("conversation-b".into())));
    assert!(
        fixture.upstreams[index].1.try_recv().is_err(),
        "重复提交或刷新不能重复调用 Provider"
    );
}

#[tokio::test]
#[ignore = "手动浏览器验收：隔离数据库与模拟 Provider，最多等待 15 分钟"]
async fn browser_model_switching_fixture() {
    let fixture = Fixture::with_pause(Duration::from_secs(20)).await;
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
