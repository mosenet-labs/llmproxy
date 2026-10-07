mod session_tests {
    use super::super::ChatSessions;
    use crate::chat_stream::ChatReply;
    use topcoat_ant_design::ChatMessageStatus;

    #[test]
    fn session_keeps_turns_in_page_history() {
        let sessions = ChatSessions::default();
        let first = sessions.create(None, "1", "openai_chat").unwrap();
        let room = sessions.get(&first).unwrap();
        assert!(room.begin("  hello  "));
        assert_eq!(room.snapshot().0[0].content, "hello");
        assert!(!room.begin("again"));
        assert!(room.start_request().is_some());
        assert!(room.start_request().is_none());
        room.update(&ChatReply {
            thinking: "thinking".to_owned(),
            ..ChatReply::default()
        });
        assert_eq!(room.snapshot().0[1].status, ChatMessageStatus::Streaming);
        assert_eq!(room.snapshot().1["2"], "thinking");
        room.update(&ChatReply {
            content: "partial".to_owned(),
            thinking: "thinking".to_owned(),
            ..ChatReply::default()
        });
        assert_eq!(room.snapshot().0[1].content, "partial");
        assert_eq!(room.snapshot().1["2"], "thinking");
        room.finish(Ok(ChatReply {
            content: "reply".to_owned(),
            thinking: "thinking done".to_owned(),
            ..ChatReply::default()
        }));
        assert_eq!(room.snapshot().0[1].content, "reply");
        assert_eq!(room.snapshot().1["2"], "thinking done");
        let next = sessions
            .create(Some(&first), "2", "openai_responses")
            .unwrap();
        assert!(sessions.get(&next).unwrap().snapshot().0.is_empty());
        assert_eq!(sessions.list(&first, &next).len(), 2);
        assert_eq!(sessions.list(&first, &next)[1].1, "hello");
        assert!(sessions.get(&first).is_some());
        let third = sessions
            .create(Some(&first), "2", "anthropic_messages")
            .unwrap();
        assert_eq!(sessions.list(&first, &third).len(), 2);
        let other_page = sessions.create(None, "3", "anthropic_messages").unwrap();
        assert_eq!(sessions.list(&other_page, &other_page).len(), 1);
    }

    #[tokio::test]
    async fn stop_before_request_is_remembered_and_next_turn_resets_it() {
        let sessions = ChatSessions::default();
        let id = sessions.create(None, "1", "openai_chat").unwrap();
        let room = sessions.get(&id).unwrap();
        assert!(!room.stop());
        assert!(room.begin("first"));
        assert!(room.stop());
        assert!(room.start_request().is_some());
        let mut cancellation = room.cancellation();
        assert!(*cancellation.wait_for(|stopped| *stopped).await.unwrap());
        assert!(!room.begin("too early"));
        room.finish(Err("已停止生成".into()));
        assert_eq!(room.snapshot().0[1].status, ChatMessageStatus::Cancelled);
        assert!(!room.snapshot().2);
        assert!(room.usage_snapshot().is_empty());
        assert!(!room.stop());
        assert!(room.begin("second"));
        assert!(!*cancellation.borrow());
        assert!(room.start_request().is_some());
        assert!(room.stop());
        assert!(*cancellation.wait_for(|stopped| *stopped).await.unwrap());
    }

    #[test]
    fn switching_keeps_identity_history_and_per_turn_stats_and_rejects_busy_changes() {
        use crate::chat_stream::history::{Content, Selection};
        use llmproxy_core::{
            ir::{
                message::{PartKind, Role},
                usage::Usage,
            },
            protocol::Protocol,
        };
        let sessions = ChatSessions::default();
        let id = sessions.create(None, "1", "openai_chat").unwrap();
        let room = sessions.get(&id).unwrap();
        let next = Selection {
            model_id: "2".into(),
            protocol: Protocol::Gemini,
        };
        assert!(room.begin("first"));
        assert!(!room.select(next.clone()));
        let first = room.start_request().unwrap();
        assert_eq!(first.selection.protocol, Protocol::OpenAiChat);
        assert!(!room.select(next.clone()));
        room.request_model("first-route");
        let mut usage = Usage {
            input_tokens: Some(10),
            ..Default::default()
        };
        usage.cache.read_input_tokens = Some(4);
        room.finish(Ok(ChatReply {
            content: "rendered answer".into(),
            history: Content::text(Role::Assistant, "structured answer"),
            model: Some("provider-model".into()),
            usage: Some(usage),
            warnings: vec!["旧轮提示".into()],
            ..Default::default()
        }));
        let labels = room.model_snapshot();
        assert!(labels["2"].contains("first-route · Chat · 返回模型 provider-model"));
        assert!(room.select(next.clone()));
        assert_eq!(sessions.list(&id, &id).len(), 1);
        assert_eq!(sessions.list(&id, &id)[0].1, "first");
        assert!(room.begin("second"));
        let turn = room.start_request().unwrap();
        assert_eq!(turn.selection, next);
        let prepared = turn.history.request(&turn.selection).body;
        assert_eq!(prepared.messages.len(), 3);
        assert_eq!(
            prepared.messages[1].parts[0].kind,
            PartKind::Text("structured answer".into())
        );
        assert_eq!(
            first.history.request(&first.selection).body.messages.len(),
            1
        );
        room.finish(Err("error".into()));
        assert_eq!(room.model_snapshot()["2"], labels["2"]);
        assert_eq!(room.usage_snapshot()["2"], "输入 10 · 缓存读取 4");
        assert_eq!(room.warning_snapshot()["2"], "旧轮提示");
        assert!(room.begin("third"));
        let turn = room.start_request().unwrap();
        assert_eq!(turn.history.request(&turn.selection).body.messages.len(), 4);
    }

    #[test]
    fn stopped_success_and_display_text_never_commit_assistant_history() {
        use crate::chat_stream::history::Content;
        use llmproxy_core::ir::message::Role;
        let sessions = ChatSessions::default();
        let id = sessions.create(None, "1", "openai_chat").unwrap();
        let room = sessions.get(&id).unwrap();
        assert!(room.begin("first"));
        assert!(room.stop());
        room.finish(Ok(ChatReply {
            content: "display".into(),
            history: Content::text(Role::Assistant, "must not commit"),
            ..Default::default()
        }));
        assert!(room.begin("second"));
        let turn = room.start_request().unwrap();
        assert_eq!(turn.history.request(&turn.selection).body.messages.len(), 2);
        room.finish(Ok(ChatReply {
            content: "display only".into(),
            ..Default::default()
        }));
        assert!(room.begin("third"));
        let turn = room.start_request().unwrap();
        assert_eq!(turn.history.request(&turn.selection).body.messages.len(), 3);
    }
}

#[cfg(test)]
mod usage_tests {
    use super::super::*;
    #[test]
    fn streaming_audio_is_cleared_on_stop_or_failure_and_never_enters_history() {
        for stopped in [false, true] {
            let room = ChatSession::new("s".into(), "m".into(), "openai_chat".into());
            assert!(room.begin("audio"));
            room.update(&ChatReply {
                parts: vec![DisplayPart {
                    title: "音频转录".into(),
                    text: "尚未完成的转录".into(),
                    media: None,
                }],
                ..Default::default()
            });
            assert!(!room.parts_snapshot().is_empty());
            if stopped {
                assert!(room.stop());
            }
            room.finish(Err("Provider 生成失败".into()));
            assert!(room.parts_snapshot().is_empty());
            assert!(room.begin("next"));
            assert!(!format!("{:?}", room.start_request().unwrap()).contains("尚未完成的转录"));
        }
    }

    #[test]
    fn streaming_tools_and_usage_update_before_completion_and_are_cleared_on_failure() {
        let room = ChatSession::new("s".into(), "m".into(), "openai_chat".into());
        assert!(room.begin("hi"));
        let usage = llmproxy_core::ir::usage::Usage {
            input_tokens: Some(10),
            ..Default::default()
        };
        room.update(&ChatReply {
            parts: vec![DisplayPart {
                title: "工具调用 · lookup".into(),
                text: "{\"q\":".into(),
                media: None,
            }],
            usage: Some(usage),
            ..Default::default()
        });
        assert!(room.snapshot().2);
        assert_eq!(room.parts_snapshot()["2"][0].text, "{\"q\":");
        assert_eq!(room.usage_snapshot()["2"], "输入 10");
        room.finish(Err("Provider 生成失败".into()));
        assert!(!room.snapshot().2);
        assert!(room.parts_snapshot().is_empty());
        assert!(room.usage_snapshot().is_empty());
    }

    #[test]
    fn usage_is_separate_from_history_and_preserves_zero() {
        let room = ChatSession::new("s".into(), "m".into(), "openai_chat".into());
        assert!(room.begin("hi"));
        let mut usage = llmproxy_core::ir::usage::Usage::default();
        usage.cache.read_input_tokens = Some(0);
        room.finish(Ok(ChatReply {
            content: "hello".into(),
            usage: Some(usage),
            ..Default::default()
        }));
        assert_eq!(room.usage_snapshot()["2"], "缓存读取 0");
        assert_eq!(room.snapshot().0[1].content, "hello");
    }

    #[test]
    fn display_blocks_do_not_replace_text_history_or_usage() {
        let room = ChatSession::new("s".into(), "m".into(), "openai_chat".into());
        assert!(room.begin("hi"));
        room.finish(Ok(ChatReply {
            content: "answer".into(),
            parts: vec![DisplayPart {
                title: "工具调用 · lookup".into(),
                text: "private-argument".into(),
                media: None,
            }],
            ..Default::default()
        }));
        assert_eq!(room.parts_snapshot()["2"][0].title, "工具调用 · lookup");
        assert_eq!(room.snapshot().0[1].content, "answer");
        assert!(room.usage_snapshot().is_empty());
        assert!(room.begin("next"));
        assert!(!format!("{:?}", room.start_request().unwrap()).contains("private-argument"));
    }
}

#[test]
fn thinking_choice_is_frozen_per_turn_and_survives_model_switch() {
    use super::*;
    use llmproxy_core::thinking::Choice;
    let room = ChatSession::new("s".into(), "m".into(), "openai_chat".into());
    assert!(room.set_thinking(&room.selection(), Choice::Enabled));
    assert!(room.begin("first"));
    assert!(!room.set_thinking(&room.selection(), Choice::Disabled));
    let first = room.start_request().unwrap();
    assert_eq!(first.thinking, Choice::Enabled);
    room.finish(Err("test".into()));
    assert!(room.select(Selection {
        model_id: "new".into(),
        protocol: Protocol::Gemini
    }));
    assert_eq!(room.thinking_choice(), Choice::Enabled);
    assert!(room.set_thinking(&room.selection(), Choice::Disabled));
    assert!(room.begin("next"));
    assert_eq!(room.start_request().unwrap().thinking, Choice::Disabled);
    assert!(room.model_snapshot()["2"].contains("思考开启"));
    assert!(room.model_snapshot()["4"].contains("思考关闭"));
}
