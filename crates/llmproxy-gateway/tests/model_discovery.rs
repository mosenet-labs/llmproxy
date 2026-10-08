mod nonstream;
mod streaming;
mod support;

use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::{ModelRouteInput, ModelRouteTargetInput, ProviderInput, ProviderPaths};
use nonstream::{ALL, Database, MASTER_KEY, alias, fixtures, path};
use std::time::{Duration, Instant};
use support::{DEADLINE, Gateway, Mock, respond, values};

fn catalog(gateway: &Gateway, path: &str) -> serde_json::Value {
    let response = gateway.request("GET", path, "", b"");
    assert_eq!(response.status, 200);
    assert_eq!(
        values(&response.headers, "content-type"),
        ["application/json"]
    );
    serde_json::from_slice(&response.body()).unwrap()
}

#[tokio::test]
async fn discovery_and_automatic_bridging_use_the_same_targets() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (upstream, requests) = Mock::http(move |_, stream| {
            respond(
                stream,
                200,
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&fixtures::response(target, false)).unwrap(),
            );
        });
        database
            .add_provider(
                ProviderInput {
                    name: target.as_str().into(),
                    paths: ProviderPaths::single(target),
                    host: "127.0.0.1".into(),
                    port: upstream.address.port(),
                    tls: false,
                    api_key: "discovery-secret".into(),
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
    let mappings = database.store.list_models().await.unwrap();
    let mapping = mappings
        .iter()
        .find(|m| m.protocols == [Protocol::OpenAiChat])
        .unwrap();
    for source in [Protocol::OpenAiChat, Protocol::OpenAiResponses] {
        database
            .store
            .create_route(ModelRouteInput {
                name: "shared-route".into(),
                protocol: source,
                provider_protocol: Protocol::OpenAiChat,
                enabled: true,
                targets: vec![ModelRouteTargetInput {
                    model_id: mapping.id,
                    enabled: true,
                }],
            })
            .await
            .unwrap();
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let list = catalog(&gateway, "/models");
    assert_eq!(list, catalog(&gateway, "/v1/models"));
    assert_eq!(list["object"], "list");
    let entries = list["data"].as_array().unwrap();
    assert_eq!(
        entries.iter().filter(|m| m["id"] == "shared-route").count(),
        1
    );
    let names: Vec<_> = entries.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    for item in entries {
        assert_eq!(item["id"], item["name"]);
        assert_eq!(item["protocols"].as_object().unwrap().len(), 4);
        assert_eq!(item["protocols"]["chat"], "/v1/chat/completions");
        assert_eq!(item["protocols"]["responses"], "/v1/responses");
        assert_eq!(item["protocols"]["messages"], "/v1/messages");
        assert_eq!(
            item["protocols"]["gemini"],
            "/v1beta/models/{model}:generateContent"
        );
    }
    assert!(!list.to_string().contains("discovery-secret"));
    assert!(upstreams.iter().all(|(upstream, _)| upstream.count() == 0));
    for method in ["POST", "HEAD", "DELETE"] {
        let response = gateway.request(method, "/models", "", b"");
        assert_eq!(response.status, 405);
        assert_eq!(values(&response.headers, "allow"), ["GET"]);
    }
    for (target, (_, requests)) in ALL.into_iter().zip(&upstreams) {
        // Neither the direct alias nor the route alias has entries for all source protocols.
        for name in [
            format!("internal-{}", target.as_str()),
            alias(Protocol::OpenAiChat, target),
        ] {
            for source in ALL {
                let response = gateway.request(
                    "POST",
                    &path(source, &name),
                    "Content-Type: application/json\r\n",
                    &serde_json::to_vec(&fixtures::request(source, &name, "text")).unwrap(),
                );
                assert_eq!(
                    response.status,
                    200,
                    "{name}: {source:?} -> {target:?}: {}",
                    gateway.logs()
                );
                let request = requests.recv_timeout(DEADLINE).unwrap();
                assert_eq!(request.target, path(target, "upstream-model"));
                let ir = fixtures::decode_request(target, &request.body);
                if target != Protocol::Gemini {
                    assert_eq!(ir.model.as_deref(), Some("upstream-model"));
                }
                assert_eq!(ir.generation.max_output_tokens, Some(128));
                fixtures::decode_response(source, &response.body()).unwrap();
            }
        }
    }
    // Definitive absence filters aliases and their routes and rejects actual inference.
    save_probe(
        &database.store,
        mapping.id,
        llmproxy_probe::Verdict::Unavailable,
    )
    .await;
    wait_for_catalog(&gateway, &mapping.alias, false).await;
    assert!(
        !catalog(&gateway, "/models")["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == "shared-route")
    );
    let before = upstreams[0].0.count();
    for source in ALL {
        let response = gateway.request(
            "POST",
            &path(source, &mapping.alias),
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&fixtures::request(source, &mapping.alias, "text")).unwrap(),
        );
        assert_eq!(response.status, 503);
    }
    assert_eq!(upstreams[0].0.count(), before);
    save_probe(
        &database.store,
        mapping.id,
        llmproxy_probe::Verdict::Available,
    )
    .await;
    wait_for_catalog(&gateway, &mapping.alias, true).await;
    save_probe(
        &database.store,
        mapping.id,
        llmproxy_probe::Verdict::Inconclusive,
    )
    .await;
    wait_for_catalog(&gateway, &mapping.alias, true).await;
    for _ in 0..2 {
        save_probe(
            &database.store,
            mapping.id,
            llmproxy_probe::Verdict::Inconclusive,
        )
        .await;
    }
    wait_for_catalog(&gateway, &mapping.alias, false).await;
    save_probe(
        &database.store,
        mapping.id,
        llmproxy_probe::Verdict::Available,
    )
    .await;
    wait_for_catalog(&gateway, &mapping.alias, true).await;
}

async fn save_probe(
    store: &llmproxy_store::ProviderStore,
    id: i64,
    verdict: llmproxy_probe::Verdict,
) {
    let job = store
        .claim_health_checks(Some((id, Protocol::OpenAiChat, 1)), 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(
        store
            .finish_health_check(
                &job,
                llmproxy_probe::ProbeResult {
                    verdict,
                    reason: if verdict == llmproxy_probe::Verdict::Unavailable {
                        Some(llmproxy_probe::Reason::ModelNotFound)
                    } else if verdict == llmproxy_probe::Verdict::Inconclusive {
                        Some(llmproxy_probe::Reason::Timeout)
                    } else {
                        None
                    },
                    http_status: None,
                    usage: None,
                    elapsed: Duration::from_millis(10),
                    thinking_mode: llmproxy_probe::ThinkingMode::DisabledRequested,
                },
                0
            )
            .await
            .unwrap()
    );
}

async fn wait_for_catalog(gateway: &Gateway, alias: &str, present: bool) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if catalog(gateway, "/models")["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == alias)
            == present
        {
            break;
        }
        assert!(Instant::now() < deadline, "catalog did not refresh");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn empty_catalog_is_a_successful_list() {
    let database = Database::new().await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    assert_eq!(
        catalog(&gateway, "/models"),
        serde_json::json!({"object":"list", "data":[]})
    );
}

#[tokio::test]
async fn automatic_bridging_preserves_streams_in_all_sixteen_directions() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (upstream, requests) = Mock::http(move |_, socket| {
            support::sse_headers(socket);
            let (start, end) = streaming::frames(target, false);
            for frame in start.into_iter().chain(end) {
                support::chunk(socket, &frame).unwrap();
            }
            support::finish_chunks(socket);
        });
        database
            .add_provider(
                streaming::provider(target, upstream.address.port(), 3000),
                target,
                "upstream-model",
            )
            .await;
        upstreams.push((upstream, requests));
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for (target, (_, requests)) in ALL.into_iter().zip(&upstreams) {
        let name = format!("internal-{}", target.as_str());
        for source in ALL {
            let mut input = fixtures::request(source, &name, "text");
            if source != Protocol::Gemini {
                input["stream"] = serde_json::json!(true);
            }
            let response = gateway.request(
                "POST",
                &streaming::path(source, &name),
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&input).unwrap(),
            );
            assert_eq!(
                response.status,
                200,
                "stream {source:?} -> {target:?}: {}",
                gateway.logs()
            );
            let mut observed = streaming::Observed::new(source);
            observed.push(source, &response.body());
            observed.finish(source);
            assert_eq!(observed.text, "你好");
            let request = requests.recv_timeout(DEADLINE).unwrap();
            assert_eq!(request.target, streaming::path(target, "upstream-model"));
        }
    }
    streaming::assert_logs(&gateway, 16);
}
