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
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    let stream = body["stream"].as_bool().unwrap_or(false)
        || request.target.contains(":streamGenerateContent");
    if stream {
        sse_headers(socket);
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
        respond(
            socket,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&fixtures::response(protocol, false)).unwrap(),
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
            let protocol = if request.target.starts_with("/v1beta/") {
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
                alias: "multi-protocol".into(),
                provider_id: provider.id,
                upstream_model_id: "upstream-model".into(),
                protocols: ALL.to_vec(),
                reference_price: None,
            })
            .await
            .unwrap();
        upstreams.push((mock, received));
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
