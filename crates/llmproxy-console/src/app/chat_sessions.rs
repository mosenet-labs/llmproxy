use std::{
    cmp::Reverse,
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::broadcast;
use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};

use crate::chat_stream::ChatReply;

const SESSION_IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);

#[derive(Default)]
pub(crate) struct ChatSessions {
    entries: Mutex<HashMap<String, (Instant, Arc<ChatSession>)>>,
}

#[derive(Default)]
struct ChatState {
    messages: Vec<ChatMessage>,
    thinking: HashMap<String, String>,
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

    pub(crate) fn snapshot(&self) -> (Vec<ChatMessage>, HashMap<String, String>, bool) {
        let state = self.state.lock().expect("chat state mutex");
        (state.messages.clone(), state.thinking.clone(), state.busy)
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
        if let Some(message) = state.messages.last_mut() {
            match result {
                Ok(reply) => {
                    let thinking = reply.visible_thinking().to_owned();
                    message.content = reply.content;
                    message.status = ChatMessageStatus::Complete;
                    let id = message.id.clone();
                    if !thinking.is_empty() {
                        state.thinking.insert(id, thinking);
                    }
                }
                Err(error) => {
                    message.content = error;
                    message.status = ChatMessageStatus::Failed;
                }
            }
        }
        state.busy = false;
        state.request_started = false;
        drop(state);
        let _ = self.changed.send(());
    }
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
}
