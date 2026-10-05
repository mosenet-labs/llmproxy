use std::{
    cmp::Reverse,
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::{broadcast, watch};
use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};

use crate::chat_stream::{ChatReply, DisplayPart};

const SESSION_IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);

#[derive(Default)]
pub(crate) struct ChatSessions {
    entries: Mutex<HashMap<String, (Instant, Arc<ChatSession>)>>,
}

#[derive(Default)]
struct ChatState {
    messages: Vec<ChatMessage>,
    thinking: HashMap<String, String>,
    usage: HashMap<String, String>,
    parts: HashMap<String, Vec<DisplayPart>>,
    title: String,
    next_id: u64,
    busy: bool,
    request_started: bool,
}

pub(crate) struct ChatSession {
    scope: String,
    model_id: String,
    protocol: String,
    created: Instant,
    state: Mutex<ChatState>,
    changed: broadcast::Sender<()>,
    cancelled: watch::Sender<bool>,
}

impl ChatSession {
    fn new(scope: String, model_id: String, protocol: String) -> Self {
        Self {
            scope,
            model_id,
            protocol,
            created: Instant::now(),
            state: Mutex::new(ChatState::default()),
            changed: broadcast::channel(32).0,
            cancelled: watch::channel(false).0,
        }
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    pub(crate) fn model_id(&self) -> &str {
        &self.model_id
    }

    pub(crate) fn protocol(&self) -> &str {
        &self.protocol
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changed.subscribe()
    }

    /// 保存停止标记，停止早于 HTTP 请求开始时也能被发送过程观察到。
    pub(crate) fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    /// 通知当前请求停止；发送过程释放 HTTP future 后统一结束这一轮。
    pub(crate) fn stop(&self) -> bool {
        let state = self.state.lock().expect("chat state mutex");
        if !state.busy {
            return false;
        }
        self.cancelled.send_replace(true);
        true
    }

    pub(crate) fn snapshot(&self) -> (Vec<ChatMessage>, HashMap<String, String>, bool) {
        let state = self.state.lock().expect("chat state mutex");
        (state.messages.clone(), state.thinking.clone(), state.busy)
    }

    /// 逐轮统计与消息内容分开存储，避免下一次请求把用量文案当作对话历史。
    pub(crate) fn usage_snapshot(&self) -> HashMap<String, String> {
        self.state.lock().expect("chat state mutex").usage.clone()
    }

    /// 展示块与发给模型的文本历史分开保存。
    pub(crate) fn parts_snapshot(&self) -> HashMap<String, Vec<DisplayPart>> {
        self.state.lock().expect("chat state mutex").parts.clone()
    }

    pub(crate) fn begin(&self, prompt: &str) -> bool {
        let prompt = prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > 4000 {
            return false;
        }
        let mut state = self.state.lock().expect("chat state mutex");
        if state.busy {
            return false;
        }
        if state.messages.is_empty() {
            let title: String = prompt.chars().take(24).collect();
            state.title = if prompt.chars().count() > 24 {
                format!("{title}…")
            } else {
                title
            };
        }
        state.busy = true;
        state.request_started = false;
        self.cancelled.send_replace(false);
        state.next_id += 1;
        let user_id = state.next_id.to_string();
        state.messages.push(ChatMessage::new(
            user_id,
            ChatBubbleRole::User,
            ChatMessageStatus::Complete,
            prompt,
        ));
        state.next_id += 1;
        let assistant_id = state.next_id.to_string();
        state.messages.push(ChatMessage::new(
            assistant_id,
            ChatBubbleRole::Assistant,
            ChatMessageStatus::Sending,
            "",
        ));
        drop(state);
        let _ = self.changed.send(());
        true
    }

    pub(crate) fn start_request(&self) -> Option<Vec<ChatMessage>> {
        let mut state = self.state.lock().expect("chat state mutex");
        if !state.busy || state.request_started {
            return None;
        }
        state.request_started = true;
        Some(state.messages.clone())
    }

    pub(crate) fn update(&self, reply: &ChatReply) {
        let mut state = self.state.lock().expect("chat state mutex");
        if let Some(message) = state.messages.last_mut() {
            message.content = reply.content.clone();
            message.status = ChatMessageStatus::Streaming;
            let id = message.id.clone();
            // 工具参数、媒体和累计用量与文字一起逐帧更新，纯工具回复也能在生成中展示。
            if !reply.parts.is_empty() {
                state.parts.insert(id.clone(), reply.parts.clone());
            }
            if let Some(usage) = reply.usage.as_ref().map(usage_label) {
                state.usage.insert(id.clone(), usage);
            }
            if !reply.visible_thinking().is_empty() {
                state
                    .thinking
                    .insert(id, reply.visible_thinking().to_owned());
            }
        }
        drop(state);
        let _ = self.changed.send(());
    }

    pub(crate) fn finish(&self, result: std::result::Result<ChatReply, String>) {
        let mut state = self.state.lock().expect("chat state mutex");
        let stopped = *self.cancelled.borrow();
        let result = if stopped {
            Err("已停止生成".to_owned())
        } else {
            result
        };
        if let Some(message) = state.messages.last_mut() {
            match result {
                Ok(reply) => {
                    let thinking = reply.visible_thinking().to_owned();
                    let usage = reply.usage.as_ref().map(usage_label);
                    message.content = reply.content;
                    message.status = ChatMessageStatus::Complete;
                    let id = message.id.clone();
                    if !thinking.is_empty() {
                        state.thinking.insert(id.clone(), thinking);
                    }
                    if let Some(usage) = usage {
                        state.usage.insert(id.clone(), usage);
                    }
                    if !reply.parts.is_empty() {
                        state.parts.insert(id, reply.parts);
                    }
                }
                Err(error) => {
                    message.content = error;
                    message.status = if stopped {
                        ChatMessageStatus::Cancelled
                    } else {
                        ChatMessageStatus::Failed
                    };
                    let id = message.id.clone();
                    // 未完成的工具和累计快照不能作为完整回复或最终用量保留。
                    state.parts.remove(&id);
                    state.usage.remove(&id);
                }
            }
        }
        state.busy = false;
        state.request_started = false;
        drop(state);
        let _ = self.changed.send(());
    }
}

/// 只展示 Provider 报告的计数，零与未报告保持不同。
fn usage_label(usage: &llmproxy_core::ir::usage::Usage) -> String {
    [
        ("输入", usage.input_tokens),
        ("输出", usage.output_tokens),
        ("总计", usage.total_tokens),
        ("缓存读取", usage.cache.read_input_tokens),
        ("缓存写入", usage.cache.write_input_tokens),
        ("推理", usage.output_details.reasoning_tokens),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|n| format!("{name} {n}")))
    .collect::<Vec<_>>()
    .join(" · ")
}

impl ChatSessions {
    pub(crate) fn create(
        &self,
        scope: Option<&str>,
        model_id: &str,
        protocol: &str,
    ) -> io::Result<String> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| io::Error::other("无法创建聊天会话"))?;
        let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let scope = scope.unwrap_or(&id).to_owned();
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("chat sessions mutex");
        entries.retain(|_, (seen, _)| now.duration_since(*seen) < SESSION_IDLE_LIMIT);
        entries.insert(
            id.clone(),
            (
                now,
                Arc::new(ChatSession::new(
                    scope,
                    model_id.to_owned(),
                    protocol.to_owned(),
                )),
            ),
        );
        Ok(id)
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<ChatSession>> {
        let mut entries = self.entries.lock().expect("chat sessions mutex");
        let (seen, session) = entries.get_mut(id)?;
        *seen = Instant::now();
        Some(session.clone())
    }

    pub(crate) fn list(
        &self,
        scope: &str,
        active_id: &str,
    ) -> Vec<(String, String, String, String)> {
        let entries = self.entries.lock().expect("chat sessions mutex");
        let mut rooms: Vec<_> = entries
            .iter()
            .filter(|(_, (_, room))| room.scope == scope)
            .filter_map(|(id, (_, room))| {
                let state = room.state.lock().expect("chat state mutex");
                if id != active_id && state.messages.is_empty() {
                    return None;
                }
                let title = state.title.clone();
                Some((
                    room.created,
                    id.clone(),
                    if title.is_empty() {
                        "新会话".to_owned()
                    } else {
                        title
                    },
                    room.model_id.clone(),
                    room.protocol.clone(),
                ))
            })
            .collect();
        rooms.sort_by_key(|room| Reverse(room.0));
        rooms
            .into_iter()
            .map(|(_, id, title, model, protocol)| (id, title, model, protocol))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ChatSessions;
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
}

#[cfg(test)]
mod usage_tests {
    use super::*;
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
