//! 归档迁移与来源统计保存重试使用隔离数据库，不产生模型调用。
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[tokio::test]
async fn archive_upgrade_keeps_existing_history_unarchived() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-archive-upgrade-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("history.sqlite3").display());
    let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
        .await
        .unwrap();
    let mut connection = store.db.connection().await.unwrap();
    toasty::sql::query("PRAGMA foreign_keys=OFF")
        .exec(&mut connection)
        .await
        .unwrap();
    toasty::sql::query("CREATE TABLE __toasty_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)").exec(&mut connection).await.unwrap();
    // 建立归档迁移之前的真实表结构，插入旧记录，再走正常启动迁移。
    for migration in super::super::migrations::SQLITE_MIGRATIONS
        .migrations()
        .iter()
        .filter(|migration| migration.id() < 202610070004)
    {
        for statement in migration.sql().split("-- #[toasty::breakpoint]") {
            toasty::sql::query(statement)
                .exec(&mut connection)
                .await
                .unwrap();
        }
        toasty::sql::query(format!(
            "INSERT INTO __toasty_migrations VALUES ({}, '{}', '2026-10-07')",
            migration.id(),
            migration.name()
        ))
        .exec(&mut connection)
        .await
        .unwrap();
    }
    let room = "00000000000000000000000000000001";
    let title = store
        .cipher
        .encrypt_bound("已有会话", &associated(room, "title"))
        .unwrap();
    let selection = Selection {
        model_id: "deleted-model".into(),
        protocol: Protocol::OpenAiChat,
    };
    let selection_json = json(&selection).unwrap();
    toasty::sql::query(format!("INSERT INTO chat_conversations (id, owner, title, selection_json, thinking, created_at, updated_at) VALUES ('{room}', 'workspace', '{title}', '{selection_json}', 'default', 1, 1)")).exec(&mut connection).await.unwrap();
    toasty::sql::query("PRAGMA foreign_keys=ON")
        .exec(&mut connection)
        .await
        .unwrap();
    drop(connection);
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    let record = store.get_chat_conversation(room).await.unwrap();
    assert_eq!(record.title, "已有会话");
    assert_eq!(record.selection, selection);
    assert!(!record.archived);
    assert_eq!(store.list_chat_conversations(0, 10).await.unwrap().len(), 1);
    store.set_chat_archived(room, true).await.unwrap();
    assert!(
        store
            .list_chat_conversations(0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .list_archived_chat_conversations(0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn failed_source_usage_save_survives_request_and_retries_from_the_shared_store() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-usage-retry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("history.sqlite3").display());
    let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let room = "00000000000000000000000000000001";
    let key = "00000000000000000000000000000002";
    let selection = Selection {
        model_id: "1".into(),
        protocol: Protocol::OpenAiChat,
    };
    store
        .create_chat_conversation(room, &selection)
        .await
        .unwrap();
    store
        .begin_chat_turn(Input {
            id: key.into(),
            conversation_id: room.into(),
            selection,
            alias: "m".into(),
            thinking: Choice::Default,
            prompt: "hello".into(),
        })
        .await
        .unwrap();
    let actual = ActualCall {
        provider_id: 1,
        provider_name: "p".into(),
        upstream_model: "m".into(),
        protocol: Protocol::OpenAiChat,
        reasoning: None,
        reported_model: None,
    };
    store
        .start_chat_call(key, "m", Protocol::OpenAiChat, &actual)
        .await
        .unwrap();
    let usage = Usage {
        input_tokens: Some(20),
        output_tokens: Some(0),
        ..Default::default()
    };
    // 克隆保留相同重试缓冲，模拟暂时不能写入数据库。
    let mut unavailable = store.clone();
    unavailable.cipher = KeyCipher::new(&STANDARD.encode([8; 32])).unwrap();
    assert!(
        unavailable
            .save_chat_usage(key, &actual, Some(&usage), UsageState::Partial)
            .await
            .is_err()
    );
    drop(unavailable);
    assert!(store.chat_usage_pending(key));
    assert!(!store.chat_turn(key).await.unwrap().call_finished);
    store.retry_chat_usage(key).await.unwrap();
    assert!(!store.chat_usage_pending(key));
    assert_eq!(store.chat_turn(key).await.unwrap().usage, Some(usage));
    store.retry_chat_usage(key).await.unwrap();
    assert_eq!(store.chat_turns(room).await.unwrap().len(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}
