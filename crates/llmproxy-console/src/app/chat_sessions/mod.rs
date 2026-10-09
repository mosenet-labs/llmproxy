#[cfg(test)]
use std::{cmp::Reverse, io};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::{broadcast, watch};
use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};

use crate::chat_stream::{
    ChatReply, DisplayPart,
    history::{Content, Conversation, Selection},
};
use llmproxy_core::{
    ir::{message::Role, usage::Usage},
    protocol::Protocol,
};

const SESSION_IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);

/// 请求选择与 IR 历史在同一次锁内取得，异步发送不再读取可变会话选择。
#[derive(Clone, Debug)]
pub(crate) struct TurnRequest {
    pub record_id: Option<String>,
    pub thinking: llmproxy_core::thinking::Choice,
    pub selection: Selection,
    pub history: Conversation,
}

struct TurnInfo {
    record_id: Option<String>,
    actual: Option<llmproxy_store::chat_history::ActualCall>,
    thinking: llmproxy_core::thinking::Choice,
    selection: Selection,
    alias: String,
    reported_model: Option<String>,
    /// 生成时保存累计快照，结束后由来源记录覆盖，不能从展示文案反向解析。
    usage: Option<Usage>,
    warnings: Vec<String>,
}
impl TurnInfo {
    /// 简略行只显示模型与输入、输出、缓存读取；未报告保留为破折号。
    fn compact_label(&self) -> String {
        let model = self
            .actual
            .as_ref()
            .and_then(|actual| actual.reported_model.as_deref())
            .filter(|model| !model.is_empty())
            .or(self
                .reported_model
                .as_deref()
                .filter(|model| !model.is_empty()))
            .or(self
                .actual
                .as_ref()
                .map(|actual| actual.upstream_model.as_str())
                .filter(|model| !model.is_empty()))
            .unwrap_or(if self.alias.is_empty() {
                "模型待确认"
            } else {
                &self.alias
            });
        let count =
            |value: Option<u64>| value.map_or_else(|| "—".into(), |value| value.to_string());
        let usage = self.usage.as_ref();
        format!(
            "{model} · in {} · out {} · cache {}",
            count(usage.and_then(|usage| usage.input_tokens)),
            count(usage.and_then(|usage| usage.output_tokens)),
            count(usage.and_then(|usage| usage.cache.read_input_tokens))
        )
    }

    /// 本轮实际模型、入口协议与路由别名保持独立。
    fn label(&self) -> String {
        let model = self
            .actual
            .as_ref()
            .and_then(|actual| actual.reported_model.as_deref())
            .or(self.reported_model.as_deref())
            .filter(|model| !model.is_empty())
            .unwrap_or(&self.alias);
        let protocol = match self.selection.protocol {
            Protocol::OpenAiChat => "Chat",
            Protocol::OpenAiResponses => "Responses",
            Protocol::AnthropicMessages => "Messages",
            Protocol::Gemini => "Gemini",
        };
        let label = if model.is_empty() {
            protocol.into()
        } else if self.alias.is_empty() || model == self.alias {
            format!("{model} · {protocol}")
        } else {
            format!("{} · {protocol} · 返回模型 {model}", self.alias)
        };
        let label = if let Some(actual) = &self.actual {
            format!(
                "{} / {} · 上游 {} · {label}",
                actual.provider_name,
                actual.upstream_model,
                actual.protocol.as_str()
            )
        } else {
            label
        };
        format!("{label} · {}", self.thinking.label())
    }
}

#[derive(Default)]
pub(crate) struct ChatSessions {
    entries: Mutex<HashMap<String, (Instant, Arc<ChatSession>)>>,
}

struct ChatState {
    pending_save: Option<(String, llmproxy_store::chat_history::Completion)>,
    latest_reply: ChatReply,
    save_error: String,
    thinking_choice: llmproxy_core::thinking::Choice,
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
    pub(crate) group_id: i64,
    #[cfg(test)]
    scope: String,
    #[cfg(test)]
    created: Instant,
    state: Mutex<ChatState>,
    changed: broadcast::Sender<()>,
    cancelled: watch::Sender<bool>,
}

impl ChatSession {
    fn new(_scope: String, model_id: String, protocol: String) -> Self {
        Self {
            group_id: 1,
            #[cfg(test)]
            scope: _scope,
            #[cfg(test)]
            created: Instant::now(),
            state: Mutex::new(ChatState {
                pending_save: None,
                latest_reply: Default::default(),
                save_error: String::new(),
                thinking_choice: Default::default(),
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

    /// 收起状态读取类型化统计，与展开后的完整标签使用同一轮次快照。
    pub(crate) fn compact_snapshot(&self) -> HashMap<String, String> {
        self.state
            .lock()
            .expect("chat state mutex")
            .turns
            .iter()
            .map(|(id, turn)| (id.clone(), turn.compact_label()))
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

    /// 偏好只影响下一轮；生成期间服务端也拒绝修改。
    pub(crate) fn set_thinking(
        &self,
        selection: &Selection,
        choice: llmproxy_core::thinking::Choice,
    ) -> bool {
        let mut state = self.state.lock().expect("chat state mutex");
        if state.busy || &state.selection != selection {
            return false;
        }
        state.thinking_choice = choice;
        drop(state);
        let _ = self.changed.send(());
        true
    }

    /// 会话切换时回显偏好，不从历史标签推断。
    pub(crate) fn thinking_choice(&self) -> llmproxy_core::thinking::Choice {
        self.state.lock().expect("chat state mutex").thinking_choice
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
        state.latest_reply = Default::default();
        state.save_error.clear();
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
        let thinking = state.thinking_choice;
        state.turns.insert(
            assistant_id.clone(),
            TurnInfo {
                record_id: None,
                actual: None,
                thinking,
                selection,
                alias: String::new(),
                reported_model: None,
                usage: None,
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
            record_id: state
                .messages
                .last()
                .and_then(|m| state.turns.get(&m.id))
                .and_then(|t| t.record_id.clone()),
            thinking: state.thinking_choice,
            selection: state.selection.clone(),
            history: state.history.clone(),
        })
    }

    pub(crate) fn update(&self, reply: &ChatReply) {
        let mut state = self.state.lock().expect("chat state mutex");
        state.latest_reply = reply.clone();
        if let Some(message) = state.messages.last_mut() {
            message.content = reply.content.clone();
            message.status = ChatMessageStatus::Streaming;
            let id = message.id.clone();
            if let Some(turn) = state.turns.get_mut(&id) {
                turn.warnings.clone_from(&reply.warnings);
                if reply.usage.is_some() {
                    turn.usage.clone_from(&reply.usage);
                }
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

    /// 关联已落库轮次，展示消息序号与业务请求标识分别保存。
    pub(crate) fn attach_record(&self, key: &str) {
        let mut state = self.state.lock().expect("chat state mutex");
        if let Some(id) = state.messages.last().map(|m| m.id.clone())
            && let Some(turn) = state.turns.get_mut(&id)
        {
            turn.record_id = Some(key.into());
        }
    }
    /// 提交前取得最近展示快照，停止后仍可保留已收到文字。
    pub(crate) fn record_snapshot(&self) -> (Option<String>, ChatReply) {
        let state = self.state.lock().expect("chat state mutex");
        let key = state
            .messages
            .last()
            .and_then(|m| state.turns.get(&m.id))
            .and_then(|t| t.record_id.clone());
        (key, state.latest_reply.clone())
    }
    /// 已提交输入但 HTTP 尚未启动时，停止可直接保存取消终态。
    pub(crate) fn request_active(&self) -> bool {
        self.state.lock().expect("chat state mutex").request_started
    }
    /// 停止状态独立于 Provider 结果，阻止晚到成功提交历史。
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }
    /// 保存失败的完整终态保留在内存，重试使用同一轮次标识。
    pub(crate) fn queue_save(
        &self,
        key: String,
        completion: llmproxy_store::chat_history::Completion,
    ) {
        self.state.lock().expect("chat state mutex").pending_save = Some((key, completion));
    }
    /// 读取待保存终态的独立副本，不在状态锁内等待数据库。
    pub(crate) fn pending_save(
        &self,
    ) -> Option<(String, llmproxy_store::chat_history::Completion)> {
        self.state
            .lock()
            .expect("chat state mutex")
            .pending_save
            .clone()
    }
    /// 成功保存后清除提示，并唤醒页面重新读取权威统计。
    pub(crate) fn saved(&self) {
        let mut state = self.state.lock().expect("chat state mutex");
        state.pending_save = None;
        state.save_error.clear();
        drop(state);
        let _ = self.changed.send(());
    }
    /// 保存错误在历史区域单独展示，不能混入发送上下文。
    pub(crate) fn save_error(&self, error: &str) {
        self.state.lock().expect("chat state mutex").save_error = error.into();
        let _ = self.changed.send(());
    }
    /// 页面独立展示保存错误，生成结果本身保持可见。
    pub(crate) fn persistence_error(&self) -> String {
        self.state
            .lock()
            .expect("chat state mutex")
            .save_error
            .clone()
    }
    /// 来源记录更新实际模型与用量；生成中的页面累计快照暂时保留。
    pub(crate) fn apply_record(&self, record: &llmproxy_store::chat_history::Turn) {
        let mut state = self.state.lock().expect("chat state mutex");
        let id = state
            .turns
            .iter()
            .find(|(_, t)| t.record_id.as_deref() == Some(&record.id))
            .map(|(id, _)| id.clone());
        if let Some(id) = id {
            if let Some(turn) = state.turns.get_mut(&id) {
                turn.actual = record.actual.clone();
                turn.usage = if record.call_finished || !record.call_started {
                    record.usage.clone()
                } else {
                    None
                };
            }
            if record.call_finished || !record.call_started {
                let mut label = record
                    .usage
                    .as_ref()
                    .map(saved_usage_label)
                    .unwrap_or_else(|| "用量未报告".into());
                if record.usage.is_some() {
                    label.push_str(&format!(" · {}", record.usage_state.label()));
                }
                if let Some(rate) = record
                    .usage
                    .as_ref()
                    .and_then(|u| u.input_tokens.zip(u.cache.read_input_tokens))
                    .filter(|(i, r)| *i > 0 && r <= i)
                    .map(|(i, r)| r as f64 / i as f64 * 100.0)
                {
                    label.push_str(&format!(" · 缓存命中率 {rate:.1}%"));
                }
                state.usage.insert(id, label);
            } else {
                state.usage.insert(id, "来源用量正在保存，尚未确认".into());
            }
        }
    }
    /// 完成活跃展示并提交有效 IR，失败只保留可见内容。
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
                        turn.usage = reply.usage;
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
                    if message.content.is_empty() {
                        message.content = error.clone();
                    }
                    let warning_id = message.id.clone();
                    message.status = if stopped {
                        ChatMessageStatus::Cancelled
                    } else {
                        ChatMessageStatus::Failed
                    };
                    let id = warning_id;
                    if let Some(turn) = state.turns.get_mut(&id) {
                        turn.warnings.push(error);
                        turn.usage = None;
                    }
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

/// 持久化详情明确显示每项缺失值，防止“未报告”被误读为零。
fn saved_usage_label(usage: &llmproxy_core::ir::usage::Usage) -> String {
    [
        ("输入", usage.input_tokens),
        ("输出", usage.output_tokens),
        ("总计", usage.total_tokens),
        ("缓存读取", usage.cache.read_input_tokens),
        ("缓存写入", usage.cache.write_input_tokens),
        ("推理", usage.output_details.reasoning_tokens),
    ]
    .into_iter()
    .map(|(name, value)| {
        format!(
            "{name} {}",
            value.map_or_else(|| "未报告".into(), |n| n.to_string())
        )
    })
    .collect::<Vec<_>>()
    .join(" · ")
}

impl ChatSessions {
    #[cfg(test)]
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

    /// 首屏恢复订阅时也考虑后台会话，当前查看的会话可能已经空闲。
    pub(crate) fn any_generating(&self, group_id: i64) -> bool {
        self.entries
            .lock()
            .expect("chat sessions mutex")
            .values()
            .any(|(_, room)| room.group_id == group_id && room.snapshot().2)
    }

    /// 加载空闲历史，签名仍保留在结构化 IR 中。
    pub(crate) fn restore(
        &self,
        group_id: i64,
        record: &llmproxy_store::chat_history::Conversation,
        turns: &[llmproxy_store::chat_history::Turn],
    ) -> Arc<ChatSession> {
        let mut restored = ChatSession::new(
            llmproxy_store::chat_history::OWNER.into(),
            record.selection.model_id.clone(),
            record.selection.protocol.as_str().into(),
        );
        restored.group_id = group_id;
        let room = Arc::new(restored);
        {
            let mut state = room.state.lock().expect("chat state mutex");
            state.title = record.title.clone();
            state.thinking_choice = record.thinking;
            for turn in turns {
                let uid = (turn.sequence * 2 - 1).to_string();
                let aid = (turn.sequence * 2).to_string();
                state.messages.push(ChatMessage::new(
                    uid,
                    ChatBubbleRole::User,
                    ChatMessageStatus::Complete,
                    &turn.prompt,
                ));
                state
                    .history
                    .append(&turn.selection, Content::text(Role::User, &turn.prompt));
                let reply = turn.reply.clone().unwrap_or_default();
                let status = match turn.status {
                    llmproxy_store::chat_history::Status::Completed
                    | llmproxy_store::chat_history::Status::Incomplete => {
                        ChatMessageStatus::Complete
                    }
                    llmproxy_store::chat_history::Status::Cancelled => ChatMessageStatus::Cancelled,
                    llmproxy_store::chat_history::Status::Generating => ChatMessageStatus::Sending,
                    _ => ChatMessageStatus::Failed,
                };
                let text = if reply.content.is_empty() {
                    turn.error.clone().unwrap_or_default()
                } else {
                    reply.content.clone()
                };
                state.messages.push(ChatMessage::new(
                    aid.clone(),
                    ChatBubbleRole::Assistant,
                    status,
                    text,
                ));
                if turn.status.usable() {
                    state.history.append(&turn.selection, reply.history.clone());
                    state.parts.insert(aid.clone(), reply.parts.clone());
                }
                state
                    .thinking
                    .insert(aid.clone(), reply.visible_thinking().into());
                let mut warnings = reply.warnings.clone();
                if let Some(error) = &turn.error {
                    warnings.push(error.clone());
                }
                state.turns.insert(
                    aid,
                    TurnInfo {
                        record_id: Some(turn.id.clone()),
                        actual: turn.actual.clone(),
                        thinking: turn.thinking,
                        selection: turn.selection.clone(),
                        alias: turn.alias.clone(),
                        reported_model: reply.model,
                        usage: None,
                        warnings,
                    },
                );
                state.next_id = (turn.sequence * 2) as u64;
            }
        }
        for turn in turns {
            room.apply_record(turn);
        }
        let mut entries = self.entries.lock().expect("chat sessions mutex");
        let now = Instant::now();
        entries.retain(|_, (seen, room)| {
            now.duration_since(*seen) < SESSION_IDLE_LIMIT
                || room.snapshot().2
                || room.pending_save().is_some()
        });
        entries
            .entry(record.id.clone())
            .or_insert_with(|| (Instant::now(), room))
            .1
            .clone()
    }
    /// 删除持久化会话后同步移除缓存。
    pub(crate) fn remove(&self, id: &str) {
        self.entries.lock().expect("chat sessions mutex").remove(id);
    }
    #[cfg(test)]
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
