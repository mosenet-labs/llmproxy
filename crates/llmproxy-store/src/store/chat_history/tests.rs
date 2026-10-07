//! 来源统计失败重试复用连接池与业务标识，不产生第二次模型调用。
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
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
