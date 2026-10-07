//! 保存失败重试、停止与结构化恢复使用隔离 SQLite。
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::{
    conversation::Content,
    ir::message::{PartKind, Role},
    protocol::Protocol,
};

#[tokio::test]
async fn saved_history_rehydrates_ir_and_failed_save_can_retry_without_a_provider() {
    let directory =
        std::env::temp_dir().join(format!("llmproxy-history-service-{}", new_id().unwrap()));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("history.sqlite3").display());
    let key = STANDARD.encode([7; 32]);
    let store = ProviderStore::connect(&url, &key).await.unwrap();
    store.migrate().await.unwrap();
    let sessions = ChatSessions::default();
    let service = ChatService {
        store: &store,
        sessions: &sessions,
    };
    let selection = Selection {
        model_id: "deleted-id".into(),
        protocol: Protocol::OpenAiChat,
    };
    let id = service.create(selection.clone()).await.unwrap();
    assert!(service.begin(&id, "first", "alias").await.unwrap());
    let room = service.load(&id).await.unwrap();
    room.start_request().unwrap();
    let wrong = ProviderStore::connect(&url, &STANDARD.encode([8; 32]))
        .await
        .unwrap();
    let faulty = ChatService {
        store: &wrong,
        sessions: &sessions,
    };
    faulty
        .finish(
            &id,
            Ok(Reply {
                content: "rendered".into(),
                history: Content::text(Role::Assistant, "IR content"),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert!(room.pending_save().is_some());
    assert!(!room.persistence_error().is_empty());
    assert_eq!(
        store.chat_turns(&id).await.unwrap()[0].status,
        Status::Generating
    );
    service.retry_save(&id).await.unwrap();
    assert!(room.pending_save().is_none());
    assert!(room.persistence_error().is_empty());
    assert_eq!(
        store.chat_turns(&id).await.unwrap()[0].status,
        Status::Completed
    );
    // 失败只保留可见片段，不将片段或错误加入下一轮 IR。
    assert!(service.begin(&id, "second", "alias").await.unwrap());
    room.start_request().unwrap();
    room.update(&Reply {
        content: "partial".into(),
        history: Content::text(Role::Assistant, "invalid IR"),
        ..Default::default()
    });
    room.stop();
    service.finish(&id, Err("stopped".into())).await.unwrap();
    let turns = store.chat_turns(&id).await.unwrap();
    assert_eq!(turns[1].status, Status::Cancelled);
    assert_eq!(turns[1].reply.as_ref().unwrap().content, "partial");
    assert!(turns[1].reply.as_ref().unwrap().history.items.is_empty());
    // 另一个缓存模拟进程重启；模型配置已不存在也能恢复和选择新协议。
    let restored = ChatSessions::default();
    let fresh = ChatService {
        store: &store,
        sessions: &restored,
    };
    let room = fresh.load(&id).await.unwrap();
    assert_eq!(room.snapshot().0.len(), 4);
    let next = Selection {
        model_id: "new-id".into(),
        protocol: Protocol::Gemini,
    };
    fresh.select(&id, next, Choice::Default).await.unwrap();
    assert!(fresh.begin(&id, "third", "new-alias").await.unwrap());
    let turn = room.start_request().unwrap();
    let input = turn.history.request(&turn.selection).body;
    let texts: Vec<_> = input
        .messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| match &p.kind {
            PartKind::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["first", "IR content", "second", "third"]);
    fresh.finish(&id, Err("failed".into())).await.unwrap();
    fresh.delete(&id).await.unwrap();
    assert!(restored.get(&id).is_none());
    std::fs::remove_dir_all(directory).unwrap();
}
