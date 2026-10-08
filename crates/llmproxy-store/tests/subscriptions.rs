use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::subscription::{Health, Registration, VERSION};
use llmproxy_store::{ProviderStore, SubscriptionRelayTarget};

#[tokio::test]
async fn registration_reconnect_admission_and_identity_are_atomic() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-subscriptions-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let store = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("nodes.sqlite3").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    exercise(&store).await;
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

async fn exercise(store: &ProviderStore) {
    let mut registration = Registration {
        version: VERSION,
        node_id: "a".repeat(64),
        node_key: "b".repeat(64),
        name: "personal".into(),
        backend: "chatgpt-oauth".into(),
        models: vec!["model-a".into()],
        concurrency: 4,
        health: Health::Unknown,
    };
    let node = store.register_subscription(&registration).await.unwrap();
    assert!(!node.enabled);
    assert!(node.provider_id.is_none());
    let target = SubscriptionRelayTarget {
        host: "127.0.0.1".into(),
        port: 8080,
        key: "relay-key".repeat(8),
    };
    store
        .set_subscription_enabled(&node.node_id, node.version, true, &target)
        .await
        .unwrap();
    let node = store.subscription_nodes().await.unwrap().pop().unwrap();
    let provider = store.get(node.provider_id.unwrap()).await.unwrap();
    assert!(provider.enabled);
    assert_eq!(provider.name, "personal");
    assert_eq!(
        provider.paths.openai_responses.as_deref(),
        Some(format!("/internal/subscriptions/{}/responses", node.node_id).as_str())
    );
    store
        .rename_subscription(&node.node_id, node.version, "edited")
        .await
        .unwrap();
    let node = store.subscription_nodes().await.unwrap().pop().unwrap();
    assert_eq!(
        store.get(node.provider_id.unwrap()).await.unwrap().name,
        "edited"
    );
    assert!(
        store
            .rename_subscription(&node.node_id, node.version, &"x".repeat(129))
            .await
            .is_err()
    );
    registration.name = "renamed".into();
    registration.models = vec!["model-b".into()];
    let reconnect = store.register_subscription(&registration).await.unwrap();
    assert!(reconnect.enabled);
    assert_eq!(reconnect.name, "personal");
    assert_eq!(reconnect.provider_name.as_deref(), Some("edited"));
    assert_eq!(reconnect.version, node.version);
    assert_eq!(reconnect.provider_id, node.provider_id);
    assert_eq!(store.list().await.unwrap().len(), 1);
    registration.node_key = "c".repeat(64);
    assert!(store.register_subscription(&registration).await.is_err());
    assert_eq!(
        store.subscription_nodes().await.unwrap()[0].name,
        "personal"
    );
    assert!(
        store
            .set_subscription_enabled(&node.node_id, node.version - 1, false, &target)
            .await
            .is_err()
    );
    store
        .set_subscription_enabled(&node.node_id, reconnect.version, false, &target)
        .await
        .unwrap();
    assert!(!store.get(node.provider_id.unwrap()).await.unwrap().enabled);
    registration.node_id = "e".repeat(64);
    registration.name = "office".into();
    let other = store.register_subscription(&registration).await.unwrap();
    store
        .rename_subscription(&other.node_id, other.version, "edited")
        .await
        .unwrap();
    let other = store.register_subscription(&registration).await.unwrap();
    store
        .set_subscription_enabled(&other.node_id, other.version, true, &target)
        .await
        .unwrap();
    let other = store.register_subscription(&registration).await.unwrap();
    assert_eq!(
        store.get(other.provider_id.unwrap()).await.unwrap().name,
        "office"
    );
    registration.node_id = "f".repeat(64);
    registration.name = "edited".into();
    let conflict = store.register_subscription(&registration).await.unwrap();
    assert!(
        store
            .rename_subscription(&conflict.node_id, conflict.version, "office")
            .await
            .is_err()
    );
    assert!(
        store
            .set_subscription_enabled(&conflict.node_id, conflict.version, true, &target)
            .await
            .is_err()
    );
    let conflict = store.register_subscription(&registration).await.unwrap();
    assert!(conflict.provider_name.is_none());
    assert!(!conflict.enabled);
    registration.node_id = "d".repeat(64);
    registration.name.clear();
    let default_node = store.register_subscription(&registration).await.unwrap();
    assert_eq!(default_node.name, "node-dddddd");
    registration.name = "reconnect-name".into();
    assert_eq!(
        store
            .register_subscription(&registration)
            .await
            .unwrap()
            .name,
        "node-dddddd"
    );
}

#[tokio::test]
#[ignore = "需要 LLMPROXY_TEST_DATABASE_URL；显式运行隔离的 PostgreSQL 节点验收"]
async fn postgresql_registration_reconnect_admission_and_identity_are_atomic() {
    let base =
        std::env::var("LLMPROXY_TEST_DATABASE_URL").expect("PostgreSQL 节点专项必须配置数据库");
    let mut admin = toasty::Db::builder().connect(&base).await.unwrap();
    let schema = format!(
        "llmproxy_node_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    toasty::sql::statement(format!("CREATE SCHEMA {schema}"))
        .exec(&mut admin)
        .await
        .unwrap();
    let separator = if base.contains('?') { '&' } else { '?' };
    let url = format!("{base}{separator}options=-c%20search_path%3D{schema}");
    let result = tokio::spawn(async move {
        let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
            .await
            .unwrap();
        store.migrate().await.unwrap();
        exercise(&store).await;
    })
    .await;
    toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
        .exec(&mut admin)
        .await
        .unwrap();
    result.unwrap();
}
