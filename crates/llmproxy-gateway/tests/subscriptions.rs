use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::subscription::{Health, Lease, Registration, ResultFrame, VERSION, Work};
use llmproxy_store::{ProviderStore, SubscriptionRelayTarget};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Process {
    child: Child,
    directory: PathBuf,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reverse_http_relay_registration_reconnect_and_console() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-subscription-http-{}-{port}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let database = format!("sqlite:{}", directory.join("nodes.sqlite3").display());
    let master = STANDARD.encode([7; 32]);
    let register_key = "registration-key".repeat(4);
    let relay_key = "relay-key".repeat(8);
    let store = ProviderStore::connect(&database, &master).await.unwrap();
    store.migrate().await.unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_llmproxy"))
        .env_clear()
        .env("LLMPROXY_DATABASE_URL", &database)
        .env("LLMPROXY_MASTER_KEY", master)
        .env("LLMPROXY_LISTEN", format!("127.0.0.1:{port}"))
        .env("LLMPROXY_SUBSCRIPTION_REGISTRATION_KEY", &register_key)
        .env("LLMPROXY_SUBSCRIPTION_RELAY_KEY", &relay_key)
        .current_dir(&directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut process = Process { child, directory };
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..400 {
        assert!(process.child.try_wait().unwrap().is_none());
        if client
            .get(format!("{base}/ui/subscriptions"))
            .send()
            .await
            .is_ok_and(|r| r.status() == 200)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "subscriptions page must exist");
    let registration = Registration {
        version: VERSION,
        node_id: "a".repeat(64),
        node_key: "b".repeat(64),
        name: "personal node".into(),
        backend: "chatgpt-oauth".into(),
        models: vec!["test-model".into(), "second-model".into()],
        concurrency: 2,
        health: Health::Unknown,
    };
    let url = format!("{base}/agents/v1/register");
    assert_eq!(
        client
            .post(&url)
            .body(serde_json::to_vec(&registration).unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = client
        .post(&url)
        .bearer_auth(&register_key)
        .body(serde_json::to_vec(&registration).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let lease: Lease = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let node = store.subscription_nodes().await.unwrap().pop().unwrap();
    assert!(!node.enabled);
    let relay = format!("{base}/internal/subscriptions/{}/responses", node.node_id);
    assert_eq!(
        client
            .post(&relay)
            .bearer_auth(&relay_key)
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    store
        .set_subscription_enabled(
            &node.node_id,
            node.version,
            true,
            &SubscriptionRelayTarget {
                host: "127.0.0.1".into(),
                port,
                key: relay_key.clone(),
            },
        )
        .await
        .unwrap();
    let response = client
        .get(format!(
            "{base}/ui/subscriptions/models?node_id={}",
            node.node_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let import_page = response.text().await.unwrap();
    assert!(import_page.contains("test-model"));
    assert!(import_page.contains("second-model"));
    let csrf_field = import_page.split("name=\"csrf\"").nth(1).unwrap();
    let csrf = csrf_field
        .split("value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let response = client
        .post(format!("{base}/ui/subscriptions/import"))
        .header("origin", &base)
        .form(&[
            ("csrf", csrf),
            ("node_id", node.node_id.as_str()),
            ("models", "second-model"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let imported = store.list_models().await.unwrap();
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].upstream_model_id, "second-model");
    let enabled_node = store.subscription_nodes().await.unwrap().pop().unwrap();
    let response = client
        .post(format!("{base}/ui/subscriptions/name"))
        .header("origin", &base)
        .form(&[
            ("csrf", csrf),
            ("node_id", node.node_id.as_str()),
            ("version", &enabled_node.version.to_string()),
            ("name", "office-node"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        store
            .get(enabled_node.provider_id.unwrap())
            .await
            .unwrap()
            .name,
        "office-node"
    );
    let failed_edit = client
        .post(format!("{base}/ui/subscriptions/name"))
        .header("origin", &base)
        .form(&[
            ("csrf", csrf),
            ("node_id", node.node_id.as_str()),
            ("version", &enabled_node.version.to_string()),
            ("name", "stale-edit"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(failed_edit.status(), 200);
    assert!(failed_edit.url().path() == "/ui/subscriptions");
    assert!(failed_edit.text().await.unwrap().contains("节点配置已变化"));
    // 四个空闲轮询共享队列，所有请求都应在一个长轮询周期内结束。
    let started = std::time::Instant::now();
    let mut idle_polls = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let client = client.clone();
        let url = format!("{base}/agents/v1/node/{}/poll", node.node_id);
        let token = lease.token.clone();
        idle_polls.spawn(async move {
            client
                .get(url)
                .bearer_auth(token)
                .timeout(Duration::from_secs(25))
                .send()
                .await
                .unwrap()
                .status()
        });
    }
    while let Some(result) = idle_polls.join_next().await {
        assert_eq!(result.unwrap(), 204);
    }
    assert!(started.elapsed() < Duration::from_secs(25));
    let sending_client = client.clone();
    let sending_relay = relay.clone();
    let sending_key = relay_key.clone();
    let request = tokio::spawn(async move {
        sending_client
            .post(sending_relay)
            .bearer_auth(sending_key)
            .body(r#"{"model":"test-model","input":"private input"}"#)
            .send()
            .await
            .unwrap()
    });
    let poll = format!("{base}/agents/v1/node/{}/poll", node.node_id);
    let response = client
        .get(&poll)
        .bearer_auth(&lease.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let work: Work = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(work.body["input"], "private input");
    let output = r#"{"id":"response-1","object":"response","status":"completed","output":[]}"#;
    let frames = [
        ResultFrame::Head {
            status: 200,
            content_type: "application/json".into(),
        },
        ResultFrame::Data {
            base64: STANDARD.encode(output),
        },
        ResultFrame::End,
    ];
    let body = frames
        .iter()
        .map(|f| format!("{}\n", serde_json::to_string(f).unwrap()))
        .collect::<String>();
    let response = client
        .post(format!(
            "{base}/agents/v1/node/{}/result/{}",
            node.node_id, work.request_id
        ))
        .bearer_auth(&lease.token)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let response = request.await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), output);
    let sending_client = client.clone();
    let sending_relay = relay.clone();
    let sending_key = relay_key.clone();
    let cancelled = tokio::spawn(async move {
        sending_client
            .post(sending_relay)
            .bearer_auth(sending_key)
            .body(r#"{"model":"test-model","input":"cancel before headers"}"#)
            .send()
            .await
    });
    let response = client
        .get(&poll)
        .bearer_auth(&lease.token)
        .send()
        .await
        .unwrap();
    let cancelling_work: Work = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    cancelled.abort();
    let _ = cancelled.await;
    let mut propagated = false;
    for _ in 0..30 {
        let response = client
            .post(format!("{base}/agents/v1/node/{}/heartbeat", node.node_id))
            .bearer_auth(&lease.token)
            .body(r#"{"health":"ready"}"#)
            .send()
            .await
            .unwrap();
        let cancellations: llmproxy_core::subscription::Cancellations =
            serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        if cancellations
            .request_ids
            .contains(&cancelling_work.request_id)
        {
            propagated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        propagated,
        "disconnect before response headers must cancel remote work"
    );
    let response = client
        .post(&url)
        .bearer_auth(&register_key)
        .body(serde_json::to_vec(&registration).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let next: Lease = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_ne!(lease.token, next.token);
    assert_eq!(
        client
            .get(&poll)
            .bearer_auth(&lease.token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(store.subscription_nodes().await.unwrap()[0].enabled);
    let page = client
        .get(format!("{base}/ui/subscriptions"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("personal node"));
    assert!(page.contains("office-node"));
    assert!(page.contains("确认停用"));
    assert!(!page.contains("保存名称"));
    assert!(!page.contains(&registration.node_key));
    assert!(!page.contains(&relay_key));
    assert_eq!(
        client
            .post(format!("{base}/ui/subscriptions/enabled"))
            .form(&[
                ("csrf", "wrong"),
                ("node_id", &node.node_id),
                ("version", "1"),
                ("enabled", "false")
            ])
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    drop(store);
}
