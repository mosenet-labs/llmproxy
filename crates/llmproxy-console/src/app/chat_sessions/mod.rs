use std::{
    cmp::Reverse,
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::{broadcast, watch};
use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};

use crate::chat_stream::{
    ChatReply, DisplayPart,
    history::{Content, Conversation, Selection},
};
use llmproxy_core::{ir::message::Role, protocol::Protocol};

const SESSION_IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);

/// 请求选择与 IR 历史在同一次锁内取得，异步发送不再读取可变会话选择。
#[derive(Clone, Debug)]
pub(crate) struct TurnRequest {
    pub selection: Selection,
    pub history: Conversation,
}

struct TurnInfo {
    selection: Selection,
    alias: String,
    reported_model: Option<String>,
    warnings: Vec<String>,
}
impl TurnInfo {
    /// 本轮实际模型、入口协议与路由别名保持独立。
    fn label(&self) -> String {
        let model = self
            .reported_model
            .as_deref()
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.alias);
        let protocol = match self.selection.protocol {
            Protocol::OpenAiChat => "Chat",
            Protocol::OpenAiResponses => "Responses",
            Protocol::AnthropicMessages => "Messages",
            Protocol::Gemini => "Gemini",
        };
        if model.is_empty() {
            protocol.into()
        } else if self.alias.is_empty() || model == self.alias {
            format!("{model} · {protocol}")
        } else {
            format!("{} · {protocol} · 返回模型 {model}", self.alias)
        }
    }
}

#[derive(Default)]
pub(crate) struct ChatSessions {
    entries: Mutex<HashMap<String, (Instant, Arc<ChatSession>)>>,
}

struct ChatState {
    selection: Selection,
    history: Conversation,
    turns: HashMap<String, TurnInfo>,
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
    created: Instant,
    state: Mutex<ChatState>,
    changed: broadcast::Sender<()>,
    cancelled: watch::Sender<bool>,
}

impl ChatSession {
    fn new(scope: String, model_id: String, protocol: String) -> Self {
        Self {
            scope,
            created: Instant::now(),
            state: Mutex::new(ChatState {
                selection: Selection {
                    model_id,
                    protocol: Protocol::parse(&protocol).expect("validated chat protocol"),
                },
                history: Conversation::default(),
                turns: HashMap::new(),
                messages: Vec::new(),
                thinking: HashMap::new(),
                usage: HashMap::new(),
                parts: HashMap::new(),
                title: String::new(),
                next_id: 0,
                busy: false,
                request_started: false,
            }),
            changed: broadcast::channel(32).0,
            cancelled: watch::channel(false).0,
        }
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    /// 原会话内改变下一轮选择，服务端也禁止在生成期间切换。
    pub(crate) fn select(&self, selection: Selection) -> bool {
        let mut state = self.state.lock().expect("chat state mutex");
        if state.busy {
            return false;
        }
        state.selection = selection;
        drop(state);
        let _ = self.changed.send(());
        true
    }

    /// 历史列表与控件读取当前选择，不暴露状态锁中的借用。
    pub(crate) fn selection(&self) -> Selection {
        self.state
            .lock()
            .expect("chat state mutex")
            .selection
            .clone()
    }

    /// 每轮模型标签独立保存，不随下一轮选择改变。
    pub(crate) fn model_snapshot(&self) -> HashMap<String, String> {
        self.state
            .lock()
            .expect("chat state mutex")
            .turns
            .iter()
            .map(|(id, turn)| (id.clone(), turn.label()))
            .collect()
    }

    /// 兼容提示与展示、发送历史分别保存。
    pub(crate) fn warning_snapshot(&self) -> HashMap<String, String> {
        self.state
            .lock()
            .expect("chat state mutex")
            .turns
            .iter()
            .map(|(id, turn)| (id.clone(), turn.warnings.join("；")))
            .collect()
    }

    /// 目标校验完成后记录实际使用的路由别名，随后才发 HTTP 请求。
    pub(crate) fn request_model(&self, alias: &str) {
        let mut state = self.state.lock().expect("chat state mutex");
        if state.busy && state.request_started {
            let id = state.messages.last().map(|message| message.id.clone());
            if let Some(turn) = id.and_then(|id| state.turns.get_mut(&id)) {
                turn.alias = alias.into();
            }
        }
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
        let selection = state.selection.clone();
        state
            .history
            .append(&selection, Content::text(Role::User, prompt));
        state.turns.insert(
            assistant_id.clone(),
            TurnInfo {
                selection,
                alias: String::new(),
                reported_model: None,
                warnings: Vec::new(),
            },
        );
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

    pub(crate) fn start_request(&self) -> Option<TurnRequest> {
        let mut state = self.state.lock().expect("chat state mutex");
        if !state.busy || state.request_started {
            return None;
        }
        state.request_started = true;
        Some(TurnRequest {
            selection: state.selection.clone(),
            history: state.history.clone(),
        })
    }

    pub(crate) fn update(&self, reply: &ChatReply) {
        let mut state = self.state.lock().expect("chat state mutex");
        if let Some(message) = state.messages.last_mut() {
            message.content = reply.content.clone();
            message.status = ChatMessageStatus::Streaming;
            let id = message.id.clone();
            if let Some(turn) = state.turns.get_mut(&id) {
                turn.warnings.clone_from(&reply.warnings);
            }
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
                    let history = reply.history.clone();
                    let thinking = reply.visible_thinking().to_owned();
                    let usage = reply.usage.as_ref().map(usage_label);
                    message.content = reply.content;
                    message.status = ChatMessageStatus::Complete;
                    let id = message.id.clone();
                    if let Some(turn) = state.turns.get_mut(&id) {
                        turn.reported_model = reply.model;
                        turn.warnings = reply.warnings;
                        let selection = turn.selection.clone();
                        state.history.append(&selection, history);
                    }
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
        if Protocol::parse(protocol).is_none() {
            return Err(io::Error::other("请选择有效的协议"));
        }
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
                    state.selection.model_id.clone(),
                    state.selection.protocol.as_str().to_owned(),
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
mod tests;
