//! 保存失败重试、停止与结构化恢复使用隔离 SQLite。
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::{
    conversation::Content,
    ir::message::{PartKind, Role},
    protocol::Protocol,
};

#[tokio::test]
async fn history_actions_preserve_other_sessions_and_restore_archives_after_restart() {
    let directory =
        std::env::temp_dir().join(format!("llmproxy-history-actions-{}", new_id().unwrap()));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("history.sqlite3").display());
    let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let sessions = ChatSessions::default();
    let service = ChatService {
        store: &store,
        sessions: &sessions,
    };
    let chat = Selection {
        model_id: "chat-model".into(),
        protocol: Protocol::OpenAiChat,
    };
    let gemini = Selection {
        model_id: "gemini-model".into(),
        protocol: Protocol::Gemini,
    };
    let current = service.create(chat.clone()).await.unwrap();
    let other = service.create(gemini.clone()).await.unwrap();
    assert!(
        service
            .change_history(&current, &other, HistoryAction::Archive)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(service.initial(gemini.clone()).await.unwrap().0, current);
    assert!(store.get_chat_conversation(&other).await.unwrap().archived);
    // 新缓存模拟重启；已归档历史可以查阅，但不会被页面初始化自动选择。
    let fresh_sessions = ChatSessions::default();
    let fresh = ChatService {
        store: &store,
        sessions: &fresh_sessions,
    };
    assert_eq!(fresh.initial(gemini.clone()).await.unwrap().0, current);
    assert_eq!(fresh.load(&other).await.unwrap().selection(), gemini);
    assert!(
        fresh
            .change_history(&current, &other, HistoryAction::Restore)
            .await
            .unwrap()
            .is_none()
    );
    let next = fresh
        .change_history(&current, &current, HistoryAction::Archive)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, (other.clone(), gemini.clone()));
    assert!(
        fresh
            .change_history(&other, &current, HistoryAction::Delete)
            .await
            .unwrap()
            .is_none()
    );
    assert!(fresh_sessions.get(&current).is_none());
    assert!(store.get_chat_conversation(&current).await.is_err());
    let next = fresh
        .change_history(&other, &other, HistoryAction::Delete)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(next.0, other);
    assert_eq!(next.1, gemini);
    assert_eq!(store.list_chat_conversations(0, 10).await.unwrap().len(), 1);
    assert!(
        store
            .list_archived_chat_conversations(0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    std::fs::remove_dir_all(directory).unwrap();
}

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
    let call = llmproxy_store::chat_history::ActualCall {
        provider_id: 1,
        provider_name: "provider".into(),
        upstream_model: "upstream-model".into(),
        protocol: Protocol::AnthropicMessages,
        reasoning: None,
        reported_model: Some("source-model".into()),
    };
    let mut source_usage = llmproxy_core::ir::usage::Usage {
        input_tokens: Some(42),
        output_tokens: Some(7),
        total_tokens: Some(49),
        ..Default::default()
    };
    source_usage.cache.read_input_tokens = Some(0);
    let request = room.record_snapshot().0.unwrap();
    store
        .start_chat_call(&request, "alias", Protocol::OpenAiChat, &call)
        .await
        .unwrap();
    store
        .finish_chat_call(
            &request,
            &call,
            Some(&source_usage),
            llmproxy_store::chat_history::UsageState::Final,
        )
        .await
        .unwrap();
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
                model: Some("client-model".into()),
                usage: Some(llmproxy_core::ir::usage::Usage {
                    input_tokens: Some(999),
                    ..Default::default()
                }),
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
    // 简略行使用来源模型与统计，不能将转换后的客户端报文当作权威数据。
    assert_eq!(
        room.compact_snapshot()["2"],
        "source-model · in 42 · out 7 · cache 0"
    );
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
    assert_eq!(
        room.compact_snapshot()["2"],
        "source-model · in 42 · out 7 · cache 0"
    );
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
