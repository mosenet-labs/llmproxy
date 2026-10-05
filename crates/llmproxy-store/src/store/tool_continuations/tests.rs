//! 密文绑定、事务容量和过期行为的 SQLite 回归；PG 集成复用公开存储接口。

use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[tokio::test]
async fn persists_across_connections_is_scoped_and_bound_to_its_record() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-tool-state-{}-{}",
        std::process::id(),
        now().unwrap()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("state.sqlite3").display());
    let key = STANDARD.encode([7; 32]);
    let store = ProviderStore::connect(&url, &key).await.unwrap();
    store.migrate().await.unwrap();
    let scope = "a".repeat(64);
    let states = ["call_a", "call_b"].map(|id| ToolContinuation {
        id: id.into(),
        scope: scope.clone(),
        payload: format!("private-signature-{id}"),
    });
    let ids: Vec<_> = states.iter().map(|s| s.id.clone()).collect();
    let save_started = now().unwrap();
    store.save_tool_continuations(&states).await.unwrap();
    let save_finished = now().unwrap();
    let huge = ToolContinuation {
        id: "call_huge".into(),
        scope: scope.clone(),
        payload: "s".repeat(MAX_BYTES as usize),
    };
    let tiny = ToolContinuation {
        id: "call_tiny".into(),
        scope: scope.clone(),
        payload: "tiny".into(),
    };
    assert!(matches!(
        store.save_tool_continuations(&[tiny, huge]).await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(
        store
            .load_tool_continuations(&ids, &scope)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(matches!(
        store
            .load_tool_continuations(&["call_tiny".into()], &scope)
            .await,
        Err(StoreError::NotFound)
    ));
    let mut connection = store.connection().await.unwrap();
    let mut rows = ToolContinuationRow::all()
        .exec(&mut connection)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .all(|r| !r.encrypted_payload.contains("private-signature"))
    );
    assert!(rows.iter().all(|r| {
        (save_started + RETENTION_SECONDS..=save_finished + RETENTION_SECONDS)
            .contains(&r.expires_at)
    }));
    drop(connection);
    drop(store);
    let store = ProviderStore::connect(&url, &key).await.unwrap();
    assert_eq!(
        store
            .load_tool_continuations(&ids, &scope)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(matches!(
        store.load_tool_continuations(&ids, &"b".repeat(64)).await,
        Err(StoreError::NotFound)
    ));
    store.touch_tool_continuations(&ids, &scope).await.unwrap();
    let mut connection = store.connection().await.unwrap();
    let swapped = rows[1].encrypted_payload.clone();
    rows[0]
        .update()
        .encrypted_payload(swapped)
        .exec(&mut connection)
        .await
        .unwrap();
    assert!(matches!(
        store.load_tool_continuations(&ids, &scope).await,
        Err(StoreError::Internal)
    ));
    for mut row in rows {
        row.update()
            .expires_at(0_i64)
            .exec(&mut connection)
            .await
            .unwrap();
    }
    assert!(matches!(
        store.touch_tool_continuations(&ids, &scope).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        store.load_tool_continuations(&ids, &scope).await,
        Err(StoreError::NotFound)
    ));
    drop(connection);
    // 成功写入时清理过期记录，允许新回合重新使用测试标识。
    store.save_tool_continuations(&states).await.unwrap();
    assert_eq!(
        store
            .load_tool_continuations(&ids, &scope)
            .await
            .unwrap()
            .len(),
        2
    );
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}
