use std::{collections::HashMap, io};

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ModelRouteView, ProviderStore};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{View, attributes, component, emit, live, view},
};
use topcoat_ant_design::icons::PLUS_OUTLINED;
use topcoat_ant_design::{
    ChatBubbleRole, ChatMessage, ChatMessageStatus, UiLanguage, chat_bubble, chat_markdown,
    chat_message_list, chat_sender, chat_think, select,
};

use crate::{app::AppState, chat_stream::chat_reply};

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Responses",
        Protocol::AnthropicMessages => "Messages",
        Protocol::Gemini => "Gemini",
    }
}

fn selected_protocol(protocol: &str) -> std::result::Result<Protocol, String> {
    Ok(match protocol {
        "openai_chat" => Protocol::OpenAiChat,
        "openai_responses" => Protocol::OpenAiResponses,
        "anthropic_messages" => Protocol::AnthropicMessages,
        "gemini" => Protocol::Gemini,
        _ => return Err("请选择有效的协议".to_owned()),
    })
}

fn route_available(route: &ModelRouteView) -> bool {
    route.enabled
        && route
            .targets
            .iter()
            .any(|target| target.enabled && target.model.provider_enabled)
}

async fn chat_target(
    store: &ProviderStore,
    selection: &str,
) -> std::result::Result<(String, Vec<Protocol>, bool), String> {
    if let Some(id) = selection.strip_prefix("route:") {
        let id = id.parse::<i64>().map_err(|_| "请选择有效的模型路由")?;
        let route = store
            .list_routes()
            .await
            .map_err(|_| "无法读取模型路由配置")?
            .into_iter()
            .find(|route| route.id == id)
            .ok_or("模型路由已不存在")?;
        let available = route_available(&route);
        Ok((route.name, vec![route.protocol], available))
    } else {
        let id = selection.parse::<i64>().map_err(|_| "请选择有效的模型")?;
        let model = store.get_model(id).await.map_err(|_| "无法读取模型配置")?;
        Ok((model.alias, model.protocols, model.provider_enabled))
    }
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
    let mut routes: Vec<_> = state
        .store
        .list_routes()
        .await?
        .into_iter()
        .filter(route_available)
        .collect();
    routes.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.protocol.as_str().cmp(b.protocol.as_str()))
    });
    let available = !models.is_empty() || !routes.is_empty();
    let first_model = models
        .first()
        .map(|model| model.id.to_string())
        .or_else(|| routes.first().map(|route| format!("route:{}", route.id)))
        .unwrap_or_default();
    let first_protocol = models
        .first()
        .and_then(|model| model.protocols.first())
        .or_else(|| routes.first().map(|route| &route.protocol))
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
    let streaming = signal(cx, || true);
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
                    let _sent = send_chat(csrf.clone(), session.get(), streaming.get()).await;
                }
                busy.set(false);
            }
        }
    }) };
    let reset_csrf = state.csrf.clone();
    let model_csrf = state.csrf.clone();
    let stop_csrf = state.csrf.clone();
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
                        <label class="mt-3 flex items-center gap-2 text-[12px] text-secondary"><input type="checkbox" :checked=$(streaming.get()) :disabled=$(busy.get()) @change=$(|event: Event| { streaming.set(event.target.checked); })>"流式输出"</label>
                        <p class="mt-2 mb-0 text-[11px] text-secondary">"流式输出会逐步显示回复。"</p>
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
                        <div class="mx-auto flex min-h-full w-full max-w-[1200px] shrink-0 flex-col">
                            chat_history(session: $(session), refresh: $(refresh))
                        </div>
                    </div>
                    <div class="bg-white px-7 pt-3 pb-3 max-[640px]:px-4">
                        <div class="mx-auto max-w-[1200px]">chat_sender(id: "chat-draft", draft: &draft, submit_attrs: submit, busy: Some(&busy), max_length: Some(4000), language: UiLanguage::ChineseSimplified, attrs: attributes! { class="chat-composer" },
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
                                    for route in &routes { <option value=(format!("route:{}", route.id))>(format!("{} · 路由 ({})", route.name, protocol_label(route.protocol)))</option> }
                                )
                                <button type="button" class="ml-3 rounded-md border border-border px-3 py-1 text-[12px] text-secondary" :hidden=$(!busy.get()) aria-label="停止生成" @click=$(async |_event: Event| {
                                    let _stopped = stop_chat(stop_csrf.clone(), session.get()).await;
                                })>"停止生成"</button>
                            </div>
                        )</div>
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
    let scope = current.scope().to_owned();
    let protocol_kind = selected_protocol(&protocol).map_err(io::Error::other)?;
    let (_, protocols, available) = chat_target(&state.store, &model_id)
        .await
        .map_err(io::Error::other)?;
    if !available || !protocols.contains(&protocol_kind) {
        return Err(io::Error::other("当前模型或协议已不可用").into());
    }
    Ok(sessions.create(Some(&scope), &model_id, &protocol)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/default-chat-protocol")]
pub async fn default_protocol(cx: &Cx, csrf: String, model_id: String) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let (_, protocols, available) = chat_target(&app_context::<AppState>(cx).store, &model_id)
        .await
        .map_err(io::Error::other)?;
    Ok(if available {
        protocols
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

/// 停止当前对话请求，不把停止说明写进下一轮发送给模型的历史。
#[procedure("/ui/_topcoat/runtime/procedures/stop-chat")]
pub async fn stop_chat(cx: &Cx, csrf: String, session_id: String) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    Ok(app_context::<AppState>(cx)
        .chat_sessions
        .get(&session_id)
        .is_some_and(|session| session.stop()))
}

#[procedure("/ui/_topcoat/runtime/procedures/send-chat")]
pub async fn send_chat(cx: &Cx, csrf: String, session_id: String, streaming: bool) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let Some(session) = state.chat_sessions.get(&session_id) else {
        return Ok(false);
    };
    let Some(history) = session.start_request() else {
        return Ok(false);
    };
    let mut cancellation = session.cancellation();
    let request = async {
        let protocol = selected_protocol(session.protocol())?;
        let (alias, protocols, available) = chat_target(&state.store, session.model_id()).await?;
        if !available || !protocols.contains(&protocol) {
            return Err("当前模型或协议已不可用".to_owned());
        }
        chat_reply(
            &state.chat_client,
            &state.gateway_origin,
            protocol,
            &alias,
            &history,
            streaming,
            |reply| session.update(reply),
        )
        .await
    };
    // 停止时销毁未完成的 reqwest future，使网关观察到客户端断开并关闭上游。
    let result = tokio::select! {
        biased;
        _ = cancellation.wait_for(|stopped| *stopped) => Err("已停止生成".to_owned()),
        result = request => result,
    };
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
    let (_, protocols, _) = chat_target(&state.store, &model_id.get())
        .await
        .map_err(io::Error::other)?;
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
            for kind in &protocols {
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
    let mut aliases: HashMap<_, _> = state
        .store
        .list_models()
        .await?
        .into_iter()
        .map(|model| (model.id.to_string(), model.alias))
        .collect();
    for route in state.store.list_routes().await? {
        aliases.insert(format!("route:{}", route.id), route.name);
    }
    let rooms: Vec<_> = state
        .chat_sessions
        .list(current.scope(), &session.get())
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
                    <span class="mt-0.5 w-full truncate text-[11px] text-[#8a94a3]">(format!("{} · {}", alias, match kind.as_str() { "openai_chat" => "Chat", "openai_responses" => "Responses", "anthropic_messages" => "Messages", "gemini" => "Gemini", _ => "" }))</span>
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
        let mut changed = room.subscribe();
        loop {
            let (messages, thinking, busy) = room.snapshot();
            let usage = room.usage_snapshot();
            let parts = room.parts_snapshot();
            let entries: Vec<_> = messages
                .into_iter()
                .map(|message| {
                    let thought = thinking.get(&message.id).cloned().unwrap_or_default();
                    let usage = usage.get(&message.id).cloned().unwrap_or_default();
                    let parts = parts.get(&message.id).cloned().unwrap_or_default();
                    (message, thought, usage, parts)
                })
                .collect();
            let token = emit! {
                if entries.is_empty() {
                    <div class="flex min-h-[300px] flex-1 flex-col justify-center">
                        <h2 class="m-0 text-[25px] font-semibold tracking-tight text-heading">"今天想聊什么？"</h2>
                        <p class="mt-2 mb-0 text-sm leading-6 text-secondary">"从下方选择模型，输入你的问题。历史会话仅保留在当前页面。"</p>
                    </div>
                } else {
                    chat_message_list(label: "聊天消息", attrs: attributes! { class="pb-2" },
                        #[key(message.id.clone())]
                        for (message, thought, usage, parts) in entries {
                            chat_message_entry(message: message, thought: thought, usage: usage, parts: parts)
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

#[component]
async fn chat_message_entry(
    cx: &Cx,
    message: ChatMessage,
    thought: String,
    usage: String,
    parts: Vec<crate::chat_stream::DisplayPart>,
) -> Result<impl View> {
    let thought_id = format!("chat-thought-{}", message.id);
    let thought_open = signal(cx, || false);
    Ok(view! {
        chat_bubble(role: message.role, status: if message.status == ChatMessageStatus::Complete { None } else { Some(message.status) }, language: UiLanguage::ChineseSimplified,
            if message.role == ChatBubbleRole::Assistant && !thought.is_empty() {
                chat_think(id: thought_id.as_str(), open: &thought_open, language: UiLanguage::ChineseSimplified, attrs: attributes! { class="mb-3" },
                    <p class="m-0 max-h-56 overflow-y-auto whitespace-pre-wrap break-words">(thought.as_str())</p>
                )
            }
            if message.role == ChatBubbleRole::User || message.status != ChatMessageStatus::Complete {
                <p class="m-0 whitespace-pre-wrap break-words">(if message.content.is_empty() { if thought.is_empty() { "正在等待回复…" } else { "正在思考…" } } else { message.content.as_str() })</p>
            } else {
                if parts.is_empty() {
                    chat_markdown(source: message.content.as_str())
                } else {
                    for part in &parts {
                        if part.title.is_empty() {
                            chat_markdown(source: part.text.as_str())
                        } else {
                            <section class="my-3 rounded-lg border border-border bg-[#f9fafc] p-3" aria-label=(part.title.as_str())>
                                <h4 class="mt-0 mb-2 text-[12px] font-semibold text-heading">(part.title.as_str())</h4>
                                if !part.text.is_empty() {<pre class="m-0 max-h-72 overflow-auto whitespace-pre-wrap break-words font-mono text-[12px] leading-5">(part.text.as_str())</pre>}
                                if let Some(media) = &part.media {
                                    if media.preview {
                                        match media.kind {
                                            llmproxy_core::ir::media::MediaKind::Image => {<img src=(media.uri.as_str()) alt=(part.title.as_str()) class="max-h-96 max-w-full rounded-md" loading="lazy">}
                                            llmproxy_core::ir::media::MediaKind::Audio => {<audio src=(media.uri.as_str()) controls="" preload="none" aria-label=(part.title.as_str()) class="max-w-full"></audio>}
                                            llmproxy_core::ir::media::MediaKind::Video => {<video src=(media.uri.as_str()) controls="" preload="none" aria-label=(part.title.as_str()) class="max-h-96 max-w-full rounded-md"></video>}
                                            llmproxy_core::ir::media::MediaKind::File => {}
                                        }
                                    }
                                    <a href=(media.uri.as_str()) download=(part.title.as_str()) target="_blank" rel="noopener noreferrer" class="mt-2 inline-block text-[12px] text-primary">"下载附件"</a>
                                }
                            </section>
                        }
                    }
                }
            }
            if !usage.is_empty() {
                <p class="mt-3 mb-0 text-[11px] text-secondary" aria-label="本轮词元用量">(usage.as_str())</p>
            }
        )
    })
}
