use std::{
    cmp::Reverse,
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use llmproxy_core::protocol::Protocol;
use tokio::sync::broadcast;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{View, attributes, emit, live, view},
};
use topcoat_ant_design::icons::PLUS_OUTLINED;
use topcoat_ant_design::{
    ChatBubbleRole, ChatMessage, ChatMessageStatus, UiLanguage, chat_bubble, chat_markdown,
    chat_message_list, chat_sender, select,
};

use crate::{app::AppState, chat_stream::stream_reply};

const SESSION_IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);

#[derive(Default)]
pub(crate) struct ChatSessions {
    entries: Mutex<HashMap<String, (Instant, Arc<ChatSession>)>>,
}

#[derive(Default)]
struct ChatState {
    messages: Vec<ChatMessage>,
    title: String,
    next_id: u64,
    busy: bool,
    request_started: bool,
}

struct ChatSession {
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

    fn snapshot(&self) -> (Vec<ChatMessage>, bool) {
        let state = self.state.lock().expect("chat state mutex");
        (state.messages.clone(), state.busy)
    }

    fn begin(&self, prompt: &str) -> bool {
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

    fn start_request(&self) -> Option<Vec<ChatMessage>> {
        let mut state = self.state.lock().expect("chat state mutex");
        if !state.busy || state.request_started {
            return None;
        }
        state.request_started = true;
        Some(state.messages.clone())
    }

    fn update(&self, content: &str) {
        let mut state = self.state.lock().expect("chat state mutex");
        if let Some(message) = state.messages.last_mut() {
            message.content = content.to_owned();
            message.status = ChatMessageStatus::Streaming;
        }
        drop(state);
        let _ = self.changed.send(());
    }

    fn finish(&self, result: std::result::Result<String, String>) {
        let mut state = self.state.lock().expect("chat state mutex");
        if let Some(message) = state.messages.last_mut() {
            match result {
                Ok(content) => {
                    message.content = content;
                    message.status = ChatMessageStatus::Complete;
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
    fn create(&self, scope: Option<&str>, model_id: &str, protocol: &str) -> io::Result<String> {
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

    fn get(&self, id: &str) -> Option<Arc<ChatSession>> {
        let mut entries = self.entries.lock().expect("chat sessions mutex");
        let (seen, session) = entries.get_mut(id)?;
        *seen = Instant::now();
        Some(session.clone())
    }

    fn list(&self, scope: &str, active_id: &str) -> Vec<(String, String, String, String)> {
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

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Responses",
        Protocol::AnthropicMessages => "Messages",
    }
}

fn selected_model(id: &str, protocol: &str) -> std::result::Result<(i64, Protocol), String> {
    let id = id.parse().map_err(|_| "请选择有效的模型")?;
    let protocol = match protocol {
        "openai_chat" => Protocol::OpenAiChat,
        "openai_responses" => Protocol::OpenAiResponses,
        "anthropic_messages" => Protocol::AnthropicMessages,
        _ => return Err("请选择有效的协议".to_owned()),
    };
    Ok((id, protocol))
}

#[page]
pub async fn chat(cx: &Cx) -> Result<impl View> {
    let state = app_context::<AppState>(cx);
    let mut models: Vec<_> = state
        .store
        .list_models()
        .await?
        .into_iter()
        .filter(|model| model.provider_enabled && !model.protocols.is_empty())
        .collect();
    models.sort_by(|a, b| a.alias.cmp(&b.alias));
    let available = !models.is_empty();
    let first_model = models
        .first()
        .map(|model| model.id.to_string())
        .unwrap_or_default();
    let first_protocol = models
        .first()
        .and_then(|model| model.protocols.first())
        .map(|protocol| protocol.as_str().to_owned())
        .unwrap_or_default();
    let initial = state
        .chat_sessions
        .create(None, &first_model, &first_protocol)?;
    let session = signal(cx, || initial);
    let model_id = signal(cx, || first_model);
    let protocol = signal(cx, || first_protocol);
    let draft = signal(cx, String::new);
    let busy = signal(cx, || false);
    let refresh = signal(cx, || 0usize);
    let csrf = state.csrf.clone();
    let submit = attributes! { cx => @submit=$(async |event: Event| {
        event.prevent_default();
        let prompt = draft.get();
        if !busy.get() {
            if !prompt.trim().is_empty() {
                busy.set(true);
                draft.set("".to_owned());
                let started = begin_chat(csrf.clone(), session.get(), prompt).await;
                if started {
                    refresh.increment();
                    let _sent = send_chat(csrf.clone(), session.get()).await;
                }
                busy.set(false);
            }
        }
    }) };
    let reset_csrf = state.csrf.clone();
    let model_csrf = state.csrf.clone();
    Ok(view! {
        <section class="chat-workspace flex h-[calc(100dvh-80px)] min-h-[560px] w-full overflow-hidden rounded-xl border border-border bg-white shadow-[0_12px_32px_-24px_rgba(16,24,40,.28)] max-[760px]:h-[calc(100dvh-180px)] max-[760px]:min-h-[650px] max-[760px]:flex-col" aria-label="模型聊天">
            <aside class="flex w-[232px] shrink-0 flex-col border-r border-[#e9edf2] bg-[#f9fafc] px-3 py-4 max-[760px]:w-full max-[760px]:border-r-0 max-[760px]:border-b max-[760px]:py-3" aria-label="聊天设置与历史会话">
                <div class="px-2 pb-4 max-[760px]:pb-2">
                    <p class="m-0 text-[11px] font-semibold tracking-[0.13em] text-[#8a94a3]">"LLMPROXY / CHAT"</p>
                    <h2 class="mt-1 mb-0 text-[17px] font-semibold tracking-tight text-heading">"对话工作区"</h2>
                </div>
                if available {
                    <button type="button" class="chat-new-button mb-5 flex h-9 w-full items-center justify-center gap-2 rounded-lg border border-[#dce4ee] bg-white text-[13px] font-medium text-heading hover:border-[#a9bdd8] hover:bg-[#f7faff] max-[760px]:mb-3" :disabled=$(busy.get()) @click=$(async |_event: Event| {
                        let next = new_chat(reset_csrf.clone(), session.get(), model_id.get(), protocol.get()).await;
                        session.set(next);
                        draft.set("".to_owned());
                        refresh.increment();
                    })>icon(data: PLUS_OUTLINED, attrs: attributes! { class="size-3.5" aria-hidden="true" })"新会话"</button>
                    <div class="border-t border-[#e9edf2] px-2 pt-4 max-[760px]:pt-3">
                        <h3 class="m-0 text-[11px] font-semibold tracking-[0.12em] text-[#8793a2]">"设置"</h3>
                        <div class="mt-3 grid grid-cols-[32px_minmax(0,1fr)] items-center gap-2">
                            <label class="text-[12px] font-medium text-secondary" for="chat-protocol">"协议"</label>
                            chat_protocol_picker(model_id: $(model_id), protocol: $(protocol), session: $(session), refresh: $(refresh), busy: $(busy))
                        </div>
                        <p class="mt-2 mb-0 pl-10 text-[11px] leading-[1.5] text-[#8a94a3]">"切换协议将开始新会话"</p>
                    </div>
                    <div class="mt-6 flex min-h-0 flex-1 flex-col border-t border-[#e9edf2] px-2 pt-4 max-[760px]:mt-3 max-[760px]:pt-3">
                        <div class="flex items-center justify-between"><h3 class="m-0 text-[11px] font-semibold tracking-[0.12em] text-[#8793a2]">"历史会话"</h3><span class="text-[11px] text-[#9aa4b0]">"本页"</span></div>
                        <div class="mt-2 min-h-0 overflow-y-auto max-[760px]:max-h-24">chat_session_list(session: $(session), model_id: $(model_id), protocol: $(protocol), draft: $(draft), refresh: $(refresh))</div>
                    </div>
                    <p class="mt-3 mb-0 px-2 text-[11px] leading-[1.5] text-[#9aa4b0] max-[760px]:hidden">"刷新或关闭页面后不保留会话"</p>
                }
            </aside>
            <div class="flex min-h-0 min-w-0 flex-1 flex-col">
                <header class="flex h-14 shrink-0 items-center border-b border-[#eef0f3] px-7 max-[640px]:px-4"><div><h1 class="m-0 text-[16px] font-semibold leading-5 text-heading">"Chat"</h1><p class="m-0 text-[11px] text-muted">"通过网关与已接入模型对话"</p></div></header>
                if available {
                    <div id="chat-scroll" class="flex min-h-0 flex-1 flex-col-reverse overflow-y-auto px-8 py-8 max-[640px]:px-4 max-[640px]:py-5">
                        <div class="mx-auto flex min-h-full w-full max-w-[820px] shrink-0 flex-col">
                            chat_history(session: $(session), refresh: $(refresh))
                        </div>
                    </div>
                    <div class="bg-white px-7 pt-3 pb-5 max-[640px]:px-4 max-[640px]:pb-4">
                        <div class="mx-auto max-w-[820px]">chat_sender(id: "chat-draft", draft: &draft, submit_attrs: submit, busy: Some(&busy), max_length: Some(4000), language: UiLanguage::ChineseSimplified, attrs: attributes! { class="chat-composer" },
                            <div class="chat-composer-model flex min-w-0 items-center">
                                select(attrs: attributes! { cx => id="chat-model" aria-label="选择聊天模型" class="min-w-0 max-w-[320px]" :value=$(model_id.get()) :disabled=$(busy.get()) @change=$(async |event: Event| {
                                    busy.set(true);
                                    let next_model = event.target.value;
                                    let next_protocol = default_protocol(model_csrf.clone(), next_model.clone()).await;
                                    if !next_protocol.is_empty() {
                                        let next = new_chat(model_csrf.clone(), session.get(), next_model.clone(), next_protocol.clone()).await;
                                        model_id.set(next_model);
                                        protocol.set(next_protocol);
                                        session.set(next);
                                        draft.set("".to_owned());
                                        refresh.increment();
                                    }
                                    busy.set(false);
                                }) },
                                    for model in &models { <option value=(model.id.to_string())>(model.alias.as_str())</option> }
                                )
                            </div>
                        )</div>
                        <p class="mx-auto mt-2 mb-0 max-w-[820px] text-right text-[11px] text-[#98a2af]">"回车发送 · Shift+回车换行"</p>
                    </div>
                } else {
                    <div class="flex flex-1 flex-col items-center justify-center px-6 text-center"><h2 class="m-0 text-lg font-medium text-heading">"暂无可聊天的模型"</h2><p class="mt-2 mb-5 text-sm text-secondary">"先在 Models 中添加模型，并启用对应 Provider。"</p><a class="inline-flex h-9 items-center rounded-md bg-primary px-4 text-sm font-medium text-white hover:bg-primary-hover" href=(href!(super::models::models))>"前往 Models"</a></div>
                }
            </div>
        </section>
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/new-chat")]
pub async fn new_chat(
    cx: &Cx,
    csrf: String,
    current_session: String,
    model_id: String,
    protocol: String,
) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let sessions = &state.chat_sessions;
    let current = sessions
        .get(&current_session)
        .ok_or_else(|| io::Error::other("聊天会话已过期，请刷新页面"))?;
    let scope = current.scope.clone();
    let (id, protocol_kind) = selected_model(&model_id, &protocol).map_err(io::Error::other)?;
    let model = state.store.get_model(id).await?;
    if !model.provider_enabled || !model.protocols.contains(&protocol_kind) {
        return Err(io::Error::other("当前模型或协议已不可用").into());
    }
    Ok(sessions.create(Some(&scope), &model_id, &protocol)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/default-chat-protocol")]
pub async fn default_protocol(cx: &Cx, csrf: String, model_id: String) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let id = model_id
        .parse()
        .map_err(|_| io::Error::other("无效的模型"))?;
    let model = app_context::<AppState>(cx).store.get_model(id).await?;
    Ok(if model.provider_enabled {
        model
            .protocols
            .first()
            .map(|protocol| protocol.as_str())
            .unwrap_or_default()
            .to_owned()
    } else {
        String::new()
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/begin-chat")]
pub async fn begin_chat(cx: &Cx, csrf: String, session_id: String, prompt: String) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let Some(session) = state.chat_sessions.get(&session_id) else {
        return Ok(false);
    };
    Ok(session.begin(&prompt))
}

#[procedure("/ui/_topcoat/runtime/procedures/send-chat")]
pub async fn send_chat(cx: &Cx, csrf: String, session_id: String) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let Some(session) = state.chat_sessions.get(&session_id) else {
        return Ok(false);
    };
    let Some(history) = session.start_request() else {
        return Ok(false);
    };
    let result = async {
        let (model_id, protocol) = selected_model(&session.model_id, &session.protocol)?;
        let model = state
            .store
            .get_model(model_id)
            .await
            .map_err(|_| "无法读取模型配置".to_owned())?;
        if !model.provider_enabled || !model.protocols.contains(&protocol) {
            return Err("当前模型或协议已不可用".to_owned());
        }
        stream_reply(
            &state.chat_client,
            &state.gateway_origin,
            protocol,
            &model.alias,
            &history,
            |content| session.update(content),
        )
        .await
    }
    .await;
    session.finish(result);
    Ok(true)
}

#[shard("/ui/_topcoat/runtime/shards/chat-protocol-picker")]
pub async fn chat_protocol_picker(
    cx: &Cx,
    model_id: Signal<String>,
    protocol: Signal<String>,
    session: Signal<String>,
    refresh: Signal<usize>,
    busy: Signal<bool>,
) -> Result<impl View> {
    let state = app_context::<AppState>(cx);
    let csrf = state.csrf.clone();
    let id = model_id
        .get()
        .parse()
        .map_err(|_| io::Error::other("无效的模型"))?;
    let model = state.store.get_model(id).await?;
    Ok(view! {
        select(attrs: attributes! { cx => id="chat-protocol" aria-label="选择模型协议" class="w-full" :value=$(protocol.get()) :disabled=$(busy.get()) @change=$(async |event: Event| {
            busy.set(true);
            let next_protocol = event.target.value;
            let next = new_chat(csrf.clone(), session.get(), model_id.get(), next_protocol.clone()).await;
            protocol.set(next_protocol);
            session.set(next);
            refresh.increment();
            busy.set(false);
        }) },
            for kind in &model.protocols {
                <option value=(kind.as_str())>(protocol_label(*kind))</option>
            }
        )
    })
}

#[shard("/ui/_topcoat/runtime/shards/chat-session-list")]
pub async fn chat_session_list(
    cx: &Cx,
    session: Signal<String>,
    model_id: Signal<String>,
    protocol: Signal<String>,
    draft: Signal<String>,
    refresh: Signal<usize>,
) -> Result<impl View> {
    let _revision = refresh.get();
    let state = app_context::<AppState>(cx);
    let current = state
        .chat_sessions
        .get(&session.get())
        .ok_or_else(|| io::Error::other("聊天会话已过期，请刷新页面"))?;
    let aliases: HashMap<_, _> = state
        .store
        .list_models()
        .await?
        .into_iter()
        .map(|model| (model.id.to_string(), model.alias))
        .collect();
    let rooms: Vec<_> = state
        .chat_sessions
        .list(&current.scope, &session.get())
        .into_iter()
        .map(|(id, title, model, kind)| {
            let alias = aliases.get(&model).cloned().unwrap_or_default();
            (id, title, model, kind, alias)
        })
        .collect();
    Ok(view! {
        <nav class="grid gap-1" aria-label="本页历史会话">
            #[key(id.clone())]
            for (id, title, model, kind, alias) in rooms {
                <button type="button" class="chat-session-item flex w-full min-w-0 flex-col items-start rounded-lg px-2.5 py-2 text-left hover:bg-[#edf2f8]" :data-active=$(session.get() == id) @click=$(|_event: Event| {
                    session.set(id.clone());
                    model_id.set(model.clone());
                    protocol.set(kind.clone());
                    draft.set("".to_owned());
                })>
                    <span class="w-full truncate text-[12px] font-medium leading-5">(title)</span>
                    <span class="mt-0.5 w-full truncate text-[11px] text-[#8a94a3]">(format!("{} · {}", alias, match kind.as_str() { "openai_chat" => "Chat", "openai_responses" => "Responses", "anthropic_messages" => "Messages", _ => "" }))</span>
                </button>
            }
        </nav>
    })
}

#[shard("/ui/_topcoat/runtime/shards/chat-history")]
pub async fn chat_history(
    cx: &Cx,
    session: Signal<String>,
    refresh: Signal<usize>,
) -> Result<impl View> {
    let _revision = refresh.get();
    let room = app_context::<AppState>(cx)
        .chat_sessions
        .get(&session.get())
        .ok_or_else(|| io::Error::other("聊天会话已过期，请刷新页面"))?;
    Ok(live! {
        let mut changed = room.changed.subscribe();
        loop {
            let (messages, busy) = room.snapshot();
            let token = emit! {
                if messages.is_empty() {
                    <div class="flex min-h-[300px] flex-1 flex-col justify-center">
                        <h2 class="m-0 text-[25px] font-semibold tracking-tight text-heading">"今天想聊什么？"</h2>
                        <p class="mt-2 mb-0 text-sm leading-6 text-secondary">"从下方选择模型，输入你的问题。历史会话仅保留在当前页面。"</p>
                    </div>
                } else {
                    chat_message_list(label: "聊天消息", attrs: attributes! { class="pb-2" },
                        #[key(message.id.clone())]
                        for message in messages {
                            chat_bubble(role: message.role, status: if message.status == ChatMessageStatus::Complete { None } else { Some(message.status) }, language: UiLanguage::ChineseSimplified,
                                if message.role == ChatBubbleRole::User || message.status != ChatMessageStatus::Complete {
                                    <p class="m-0 whitespace-pre-wrap break-words">(if message.content.is_empty() { "正在等待回复…" } else { message.content.as_str() })</p>
                                } else {
                                    chat_markdown(source: message.content.as_str())
                                }
                            )
                        }
                    )
                }
            }?;
            if !busy {
                break Ok(token);
            }
            changed.recv().await.ok();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::ChatSessions;
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
        room.update("partial");
        assert_eq!(room.snapshot().0[1].status, ChatMessageStatus::Streaming);
        room.finish(Ok("reply".to_owned()));
        assert_eq!(room.snapshot().0[1].content, "reply");
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
