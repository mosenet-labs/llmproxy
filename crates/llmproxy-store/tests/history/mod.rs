//! 两种数据库共用的历史生命周期、并发和来源统计验收。
use llmproxy_core::{
    conversation::{Content, Reply, Selection},
    ir::{
        message::{Part, PartKind, Role},
        request::Item,
        usage::Usage,
    },
    protocol::Protocol,
    thinking::Choice,
};
use llmproxy_store::{
    ProviderStore, StoreError,
    chat_history::{ActualCall, Completion, Input, Status, Summary, UsageState},
};

fn key(n: u64) -> String {
    format!("{n:032x}")
}
fn input(room: &str, n: u64, protocol: Protocol) -> Input {
    Input {
        id: key(n),
        conversation_id: room.into(),
        selection: Selection {
            model_id: "deleted-model".into(),
            protocol,
        },
        alias: "old-alias".into(),
        thinking: Choice::Enabled,
        prompt: "私密输入".into(),
    }
}
fn completion(status: Status) -> Completion {
    let mut history = Content::text(Role::Assistant, "私密回答");
    history.messages[0].parts.push(Part {
        kind: PartKind::ToolCall(llmproxy_core::ir::message::ToolCall {
            id: Some("call1".into()),
            name: "lookup".into(),
            arguments: serde_json::json!({"q":"秘密"}),
        }),
        metadata: serde_json::from_value(serde_json::json!({"thought_signature":"私密签名"}))
            .unwrap(),
    });
    history.items.push(Item::Opaque(
        serde_json::json!({"type":"reasoning","encrypted_content":"encrypted"}),
    ));
    Completion {
        status,
        reply: Reply {
            content: "私密回答".into(),
            history,
            ..Default::default()
        },
        error: None,
    }
}
fn actual() -> ActualCall {
    ActualCall {
        provider_id: 999,
        provider_name: "已删除的Provider".into(),
        upstream_model: "upstream-v1".into(),
        protocol: Protocol::AnthropicMessages,
        reasoning: None,
        reported_model: Some("reported-v2".into()),
    }
}

/// Console 和 Gateway 可按任意顺序结束，幂等保存不会丢失权威用量。
pub async fn exercise(store: &ProviderStore, url: &str, master_key: &str) {
    let room = key(1);
    let selection = Selection {
        model_id: "deleted-model".into(),
        protocol: Protocol::OpenAiChat,
    };
    store
        .create_chat_conversation(&room, &selection)
        .await
        .unwrap();
    store
        .select_chat(&room, &selection, Choice::Enabled)
        .await
        .unwrap();
    let first = store
        .begin_chat_turn(input(&room, 10, Protocol::OpenAiChat))
        .await
        .unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(
        store
            .begin_chat_turn(input(&room, 10, Protocol::OpenAiChat))
            .await
            .unwrap()
            .sequence,
        1
    );
    let mut changed = input(&room, 10, Protocol::OpenAiChat);
    changed.thinking = Choice::Disabled;
    assert!(matches!(
        store.begin_chat_turn(changed).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(store.delete_chat_conversation(&room).await.is_err());
    assert!(store.set_chat_archived(&room, true).await.is_err());
    assert!(
        store
            .select_chat(&room, &selection, Choice::Default)
            .await
            .is_err()
    );
    let a = actual();
    assert!(
        store
            .start_chat_call(&first.id, "wrong", Protocol::OpenAiChat, &a)
            .await
            .is_err()
    );
    store
        .start_chat_call(&first.id, "old-alias", Protocol::OpenAiChat, &a)
        .await
        .unwrap();
    assert!(
        store
            .start_chat_call(&first.id, "old-alias", Protocol::OpenAiChat, &a)
            .await
            .is_err()
    );
    // Console 先保存正文，晚到的来源统计补全；重复结束正文不会覆盖。
    let final_reply = completion(Status::Completed);
    let expected = final_reply.reply.history.clone();
    store
        .finish_chat_turn(&first.id, final_reply)
        .await
        .unwrap();
    let mut usage = Usage {
        input_tokens: Some(100),
        output_tokens: Some(10),
        total_tokens: Some(110),
        ..Default::default()
    };
    usage.cache.read_input_tokens = Some(60);
    usage.cache.write_input_tokens = Some(30);
    usage.cache.write_short_input_tokens = Some(10);
    usage.cache.write_long_input_tokens = Some(20);
    usage.output_details.reasoning_tokens = Some(0);
    store
        .finish_chat_call(&first.id, &a, Some(&usage), UsageState::Final)
        .await
        .unwrap();
    store
        .finish_chat_call(&first.id, &a, None, UsageState::Unreported)
        .await
        .unwrap();
    store
        .finish_chat_turn(&first.id, completion(Status::Failed))
        .await
        .unwrap();
    let reopened = ProviderStore::connect(url, master_key).await.unwrap();
    let loaded = reopened.chat_turn(&first.id).await.unwrap();
    assert_eq!(loaded.status, Status::Completed);
    assert_eq!(loaded.reply.unwrap().history, expected);
    assert_eq!(loaded.usage, Some(usage.clone()));
    assert_eq!(
        loaded.actual.unwrap().reported_model.as_deref(),
        Some("reported-v2")
    );
    // 并发争抢同一会话只创建一轮，数据库而非浏览器负责互斥。
    store
        .select_chat(
            &room,
            &Selection {
                protocol: Protocol::Gemini,
                ..selection.clone()
            },
            Choice::Enabled,
        )
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        store.begin_chat_turn(input(&room, 11, Protocol::Gemini)),
        reopened.begin_chat_turn(input(&room, 12, Protocol::Gemini))
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let second = left.or(right).unwrap();
    assert_eq!(second.sequence, 2);
    store
        .start_chat_call(&second.id, "old-alias", Protocol::Gemini, &a)
        .await
        .unwrap();
    let mut partial = Usage {
        input_tokens: Some(300),
        ..Default::default()
    };
    partial.cache.read_input_tokens = Some(30);
    // Gateway 先保存统计，取消保留已知消耗而非补零。
    store
        .finish_chat_call(&second.id, &a, Some(&partial), UsageState::Partial)
        .await
        .unwrap();
    store
        .finish_chat_turn(
            &second.id,
            Completion {
                status: Status::Cancelled,
                reply: Reply {
                    content: "半段回答".into(),
                    ..Default::default()
                },
                error: Some("已停止生成".into()),
            },
        )
        .await
        .unwrap();
    store
        .select_chat(
            &room,
            &Selection {
                protocol: Protocol::OpenAiResponses,
                ..selection.clone()
            },
            Choice::Enabled,
        )
        .await
        .unwrap();
    let third = store
        .begin_chat_turn(input(&room, 13, Protocol::OpenAiResponses))
        .await
        .unwrap();
    store.recover_chat_history().await.unwrap();
    assert_eq!(
        store.chat_turn(&third.id).await.unwrap().status,
        Status::Interrupted
    );
    assert!(
        store
            .get_chat_conversation(&room)
            .await
            .unwrap()
            .active_turn
            .is_none()
    );
    let turns = reopened.chat_turns(&room).await.unwrap();
    assert_eq!(turns.len(), 3);
    let summary = Summary::from_turns(&turns);
    assert_eq!(summary.totals.input.total, Some(400));
    assert_eq!(summary.totals.output.total, Some(10));
    assert_eq!(summary.totals.output.reported, 1);
    assert_eq!(summary.totals.reasoning.total, Some(0));
    assert_eq!(summary.totals.partial, 1);
    assert_eq!(summary.totals.unreported, 1);
    assert_eq!(summary.totals.hit_rate(), Some(22.5));
    assert_eq!(summary.models.len(), 1);
    assert_eq!(summary.models[0].totals.turns, 2);
    // 错误主密钥既不能读元数据，也不能创建历史。
    let wrong = ProviderStore::connect(
        url,
        &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [8; 32]),
    )
    .await
    .unwrap();
    assert!(wrong.list_chat_conversations(0, 10).await.is_err());
    assert!(wrong.chat_turns(&room).await.is_err());
    assert!(wrong.set_chat_archived(&room, true).await.is_err());
    assert!(
        wrong
            .create_chat_conversation(&key(3), &selection)
            .await
            .is_err()
    );
    // 密文不含输入、回复、签名；文本统计列保留零和 NULL 的区别。
    let mut raw = toasty::Db::builder().connect(url).await.unwrap();
    let rows=toasty::sql::query("SELECT encrypted_prompt, encrypted_reply, reasoning_tokens, output_tokens FROM chat_turns ORDER BY sequence").exec(&mut raw).await.unwrap();
    let titles = toasty::sql::query("SELECT title FROM chat_conversations")
        .exec(&mut raw)
        .await
        .unwrap();
    let dump = format!("{rows:?} {titles:?}");
    for secret in ["私密输入", "私密回答", "私密签名"] {
        assert!(!dump.contains(secret));
    }
    let room2 = key(2);
    store
        .create_chat_conversation(&room2, &selection)
        .await
        .unwrap();
    assert_eq!(store.list_chat_conversations(0, 1).await.unwrap().len(), 1);
    assert_eq!(store.list_chat_conversations(1, 1).await.unwrap().len(), 1);
    // 归档只改变列表分组；跨连接读取仍保留正文、模型、用量和选择。
    let before = store.get_chat_conversation(&room).await.unwrap();
    assert!(!before.archived);
    store.set_chat_archived(&room, true).await.unwrap();
    store.set_chat_archived(&room, true).await.unwrap();
    let archived = reopened.get_chat_conversation(&room).await.unwrap();
    assert!(archived.archived);
    assert_eq!(archived.title, before.title);
    assert_eq!(archived.selection, before.selection);
    assert_eq!(
        reopened.chat_turn(&first.id).await.unwrap().usage,
        Some(usage)
    );
    assert_eq!(
        store.list_chat_conversations(0, 10).await.unwrap()[0].id,
        room2
    );
    assert_eq!(
        store.list_archived_chat_conversations(0, 1).await.unwrap()[0].id,
        room
    );
    assert!(
        store
            .list_archived_chat_conversations(1, 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .begin_chat_turn(input(&room, 15, Protocol::OpenAiChat))
            .await
            .is_err()
    );
    assert!(
        store
            .select_chat(&room, &selection, Choice::Default)
            .await
            .is_err()
    );
    reopened.set_chat_archived(&room, false).await.unwrap();
    assert_eq!(store.list_chat_conversations(0, 10).await.unwrap().len(), 2);
    assert!(
        store
            .list_archived_chat_conversations(0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    // 归档与另一页开始生成竞争，同一会话只能有一项成功。
    let (archive, begin) = tokio::join!(
        store.set_chat_archived(&room, true),
        reopened.begin_chat_turn(input(&room, 16, Protocol::OpenAiChat)),
    );
    assert_ne!(archive.is_ok(), begin.is_ok());
    if let Ok(turn) = begin {
        store
            .finish_chat_turn(&turn.id, completion(Status::Completed))
            .await
            .unwrap();
        store.set_chat_archived(&room, true).await.unwrap();
    }
    // 归档历史仍可删除，其他会话保持不变。
    store.delete_chat_conversation(&room).await.unwrap();
    assert!(store.chat_turn(&first.id).await.is_err());
    assert_eq!(store.list_chat_conversations(0, 10).await.unwrap().len(), 1);
    store.delete_chat_conversation(&room2).await.unwrap();
}
