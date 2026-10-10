use super::*;
use crate::{
    ModelRouteTargetInput, ProviderPaths, VirtualKeyInput,
    auth::{EmailPurpose, UserRole},
};
use base64::{Engine, engine::general_purpose::STANDARD};

const PASSWORD: &str = "isolated-test-password";

async fn user(store: &ProviderStore, email: &str) -> i64 {
    let code = store
        .issue_email_code(email, EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    store
        .register_user(email, &code.code, PASSWORD, "")
        .await
        .unwrap();
    store
        .login(email, PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .session
        .user
        .id
}

fn provider() -> ProviderInput {
    ProviderInput {
        name: "personal-provider".into(),
        paths: ProviderPaths::single(Protocol::OpenAiChat),
        host: "example.com".into(),
        port: 443,
        tls: true,
        api_key: "private-upstream-key".into(),
        enabled: true,
        models_path: "/models".into(),
        models_protocol: Protocol::OpenAiChat,
        anthropic_version: None,
        messages_auth: MessagesAuth::ApiKey,
        connect_timeout_ms: 1000,
        read_timeout_ms: 1000,
        write_timeout_ms: 1000,
    }
}
fn model(provider_id: i64) -> ModelMappingInput {
    ModelMappingInput {
        alias: "personal-model".into(),
        provider_id,
        upstream_model_id: "upstream".into(),
        protocols: vec![Protocol::OpenAiChat],
        reference_price: None,
        thinking: Default::default(),
    }
}
fn route(model_id: i64) -> ModelRouteInput {
    ModelRouteInput {
        name: "personal-route".into(),
        protocol: Protocol::OpenAiChat,
        provider_protocol: Protocol::OpenAiChat,
        enabled: true,
        targets: vec![ModelRouteTargetInput {
            model_id,
            enabled: true,
        }],
    }
}
async fn version(store: &ProviderStore) -> u64 {
    store
        .list_groups()
        .await
        .unwrap()
        .into_iter()
        .find(|group| group.id == store.group_id())
        .unwrap()
        .version
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn personal_catalog_keys_and_history_are_isolated() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-personal-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let store = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("db.sqlite").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let admin = store
        .bootstrap_admin("admin@example.test", PASSWORD)
        .await
        .unwrap();
    let legacy = store.create(provider()).await.unwrap();
    let legacy_model = store.create_model(model(legacy.id)).await.unwrap();
    let first_id = user(&store, "first@example.test").await;
    let second_id = user(&store, "second@example.test").await;
    let (first, same_first, second) = tokio::join!(
        store.for_user(first_id),
        store.for_user(first_id),
        store.for_user(second_id)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.group_id(), same_first.unwrap().group_id());
    assert_ne!(first.group_id(), second.group_id());
    assert!(first.list().await.unwrap().is_empty());
    assert!(first.list_all_models().await.unwrap().is_empty());
    assert_eq!(first.list_groups().await.unwrap().len(), 1);
    assert_eq!(first.list_groups().await.unwrap()[0].name, "个人资源");
    first.create_group("shared-name").await.unwrap();
    second.create_group("shared-name").await.unwrap();
    let p1 = first.create(provider()).await.unwrap();
    let p2 = second.create(provider()).await.unwrap();
    assert!(matches!(
        first.create(provider()).await,
        Err(StoreError::Conflict(_))
    ));
    for id in [legacy.id, p2.id] {
        assert!(matches!(first.get(id).await, Err(StoreError::NotFound)));
        assert!(matches!(
            first.probe_target(id).await,
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            first.set_enabled(id, p2.version, false).await,
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            first.preview_target(Some(id), provider()).await,
            Err(StoreError::NotFound)
        ));
    }
    assert!(
        first
            .activate(p1.id, p1.version, Protocol::OpenAiChat)
            .await
            .is_err()
    );
    let m1 = first.create_model(model(p1.id)).await.unwrap();
    let m2 = second.create_model(model(p2.id)).await.unwrap();
    assert!(m1.group_ids.is_empty());
    assert!(first.list_models().await.unwrap().is_empty());
    assert_eq!(first.list_all_models().await.unwrap().len(), 1);
    assert!(first.get_model(legacy_model.id).await.is_err());
    assert!(first.create_model(model(p2.id)).await.is_err());
    assert!(
        first
            .update_model(m1.id, m1.version, model(p2.id))
            .await
            .is_err()
    );
    assert!(first.delete_model(m2.id, m2.version).await.is_err());
    assert!(first.model_health_checks(m2.id).await.is_err());
    assert!(first.get_price_plan(p2.id, "upstream").await.is_err());
    let r1 = first.create_route(route(m1.id)).await.unwrap();
    let r2 = second.create_route(route(m2.id)).await.unwrap();
    assert!(first.create_route(route(m2.id)).await.is_err());
    assert!(
        first
            .update_route(r1.id, r1.version, route(m2.id))
            .await
            .is_err()
    );
    assert!(first.delete_route(r2.id, r2.version).await.is_err());
    first
        .set_group_resources(version(&first).await, vec![m1.id], vec![r1.id])
        .await
        .unwrap();
    for (models, routes) in [(vec![m2.id], vec![]), (vec![], vec![r2.id])] {
        assert!(
            first
                .set_group_resources(version(&first).await, models, routes)
                .await
                .is_err()
        );
        assert_eq!(first.list_models().await.unwrap().len(), 1);
        assert_eq!(first.list_routes().await.unwrap().len(), 1);
    }
    let key = first
        .create_virtual_key(VirtualKeyInput {
            name: "application".into(),
            all_routes: true,
            route_ids: vec![],
            model_ids: vec![],
            expires_at: None,
        })
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_some()
    );
    let forged = second.for_group(first.group_id());
    assert!(forged.list_virtual_keys().await.is_err());
    assert!(
        forged
            .set_virtual_key_enabled(key.view.id, key.view.version, false)
            .await
            .is_err()
    );
    assert!(forged.list_models().await.is_err());
    assert!(forged.list_routes().await.is_err());
    assert!(forged.load_model_routes().await.is_err());
    assert!(
        forged
            .set_group_resources(version(&first).await, vec![], vec![])
            .await
            .is_err()
    );
    assert!(
        second
            .update_group(first.group_id(), version(&first).await, "stolen", false)
            .await
            .is_err()
    );
    let selection = llmproxy_core::conversation::Selection {
        model_id: m1.id.to_string(),
        protocol: Protocol::OpenAiChat,
    };
    let conversation_id = "a".repeat(32);
    first
        .create_chat_conversation(&conversation_id, &selection)
        .await
        .unwrap();
    assert!(
        second
            .get_chat_conversation(&conversation_id)
            .await
            .is_err()
    );
    assert!(
        forged
            .get_chat_conversation(&conversation_id)
            .await
            .is_err()
    );
    assert!(forged.list_chat_conversations(0, 10).await.is_err());
    assert!(
        second
            .list_chat_conversations(0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let current = store.get_user(first_id).await.unwrap();
    store
        .update_user(
            admin.id,
            first_id,
            current.version,
            &current.display_name,
            UserRole::User,
            false,
        )
        .await
        .unwrap();
    assert!(
        store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.for_user(first_id).await.is_err());
    store.migrate().await.unwrap();
    drop((store, first, second, forged));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn subscription_claim_requires_secret_and_preserves_owner_on_reconnect() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-personal-node-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let store = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("db.sqlite").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let first = store
        .for_user(user(&store, "first@example.test").await)
        .await
        .unwrap();
    let second = store
        .for_user(user(&store, "second@example.test").await)
        .await
        .unwrap();
    let registration = llmproxy_core::subscription::Registration {
        version: llmproxy_core::subscription::VERSION,
        node_id: "a".repeat(64),
        node_key: "b".repeat(64),
        name: "my-node".into(),
        backend: "codex".into(),
        models: vec!["upstream".into()],
        concurrency: 1,
        health: llmproxy_core::subscription::Health::Ready,
    };
    store.register_subscription(&registration).await.unwrap();
    assert!(first.subscription_nodes().await.unwrap().is_empty());
    assert!(
        first
            .claim_subscription(&registration.node_id, "wrong-secret")
            .await
            .is_err()
    );
    first
        .claim_subscription(&registration.node_id, &registration.node_key)
        .await
        .unwrap();
    assert!(
        second
            .claim_subscription(&registration.node_id, &registration.node_key)
            .await
            .is_err()
    );
    assert!(second.subscription_nodes().await.unwrap().is_empty());
    store.register_subscription(&registration).await.unwrap();
    let node = first.subscription_nodes().await.unwrap().remove(0);
    assert!(
        second
            .rename_subscription(&node.node_id, node.version, "hijacked")
            .await
            .is_err()
    );
    let relay = crate::SubscriptionRelayTarget {
        host: "localhost".into(),
        port: 3000,
        key: "r".repeat(32),
    };
    first
        .set_subscription_enabled(&node.node_id, node.version, true, &relay)
        .await
        .unwrap();
    assert_eq!(first.list().await.unwrap().len(), 1);
    assert!(second.list().await.unwrap().is_empty());
    let relay_provider = first.list().await.unwrap().remove(0);
    let mut redirected = provider();
    redirected.api_key.clear();
    assert!(
        first
            .update(
                relay_provider.id,
                relay_provider.version,
                redirected.clone()
            )
            .await
            .is_err()
    );
    assert!(
        first
            .preview_target(Some(relay_provider.id), redirected)
            .await
            .is_err()
    );
    let probe = first.probe_target(relay_provider.id).await.unwrap();
    assert_eq!(probe.host, relay.host);
    assert_eq!(probe.secret, relay.key);
    assert!(
        second
            .set_subscription_enabled(&node.node_id, node.version, false, &relay)
            .await
            .is_err()
    );
    drop((store, first, second));
    std::fs::remove_dir_all(directory).unwrap();
}
