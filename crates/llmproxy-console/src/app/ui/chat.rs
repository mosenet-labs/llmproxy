// 聊天 shard 使用独立 Signal 参数共享工作区状态。
#![allow(clippy::too_many_arguments)]

use std::{collections::HashMap, io};

use llmproxy_core::{
    protocol::Protocol,
    thinking::{Choice, Config, Support},
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, component, emit, live, view},
};
use topcoat_ant_design::icons::PLUS_OUTLINED;
use topcoat_ant_design::{
    ChatBubbleRole, ChatMessage, ChatMessageStatus, NativeDialogConfig, UiLanguage, anchored_menu,
    anchored_menu_trigger_attributes, chat_bubble, chat_markdown, chat_message_list, chat_sender,
    chat_think, native_dialog, native_dialog_close_attributes, popconfirm, select,
};

use crate::{
    app::{
        AppState,
        chat_service::{ChatService, HistoryAction, target::resolve as chat_target},
    },
    chat_stream::history::Selection,
};

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Responses",
        Protocol::AnthropicMessages => "Messages",
        Protocol::Gemini => "Gemini",
    }
}

fn selected_protocol(protocol: &str) -> std::result::Result<Protocol, String> {
    Protocol::parse(protocol).ok_or_else(|| "请选择有效的协议".to_owned())
}

#[page]
pub async fn chat(cx: &Cx) -> Result<impl View> {
    let state = app_context::<AppState>(cx);
    let mut models: Vec<_> = crate::app::group_store(cx, "chat")
        .list_models()
        .await?
        .into_iter()
        .filter(|model| !model.protocols.is_empty())
        .collect();
    models.sort_by(|a, b| a.alias.cmp(&b.alias));
    let mut routes: Vec<_> = crate::app::group_store(cx, "chat")
        .list_routes()
        .await?
        .into_iter()
        .collect();
    routes.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.protocol.as_str().cmp(b.protocol.as_str()))
    });
    let available = !models.is_empty() || !routes.is_empty();
    let mut first_model = String::new();
    let mut first_protocol = models
        .first()
        .and_then(|model| model.protocols.first())
        .or_else(|| routes.first().map(|route| &route.protocol))
        .map(|protocol| protocol.as_str().to_owned())
        .unwrap_or_else(|| Protocol::OpenAiChat.as_str().to_owned());
    for id in models
        .iter()
        .map(|model| model.id.to_string())
        .chain(routes.iter().map(|route| format!("route:{}", route.id)))
    {
        let protocol = crate::app::chat_service::target::choose_protocol(
            &crate::app::group_store(cx, "chat"),
            &id,
            &first_protocol,
        )
        .await
        .map_err(io::Error::other)?;
        if !protocol.is_empty() {
            first_model = id;
            first_protocol = protocol;
            break;
        }
    }
    let service = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    };
    let (initial, selected) = service
        .initial(Selection {
            model_id: first_model,
            protocol: selected_protocol(&first_protocol).map_err(io::Error::other)?,
        })
        .await?;
    let initial_busy = service.load(&initial).await?.snapshot().2;
    let resume = state
        .chat_sessions
        .any_generating(crate::app::group_store(cx, "chat").group_id());
    let session = signal(cx, || initial);
    let model_id = signal(cx, || selected.model_id);
    let protocol = signal(cx, || selected.protocol.as_str().to_owned());
    let history_page = signal(cx, || 0usize);
    let archived_only = signal(cx, || false);
    let session_archived = signal(cx, || false);
    let draft = signal(cx, String::new);
    // 草稿只保存在当前页面，按会话 ID 隔离，不写入已经提交的历史轮次。
    let drafts = signal(cx, Vec::<(String, String)>::new);
    let selection_error = signal(cx, String::new);
    let busy = signal(cx, || false);
    // busy 只锁住短暂的页面操作，generating 仅描述当前查看的会话。
    let generating = signal(cx, || initial_busy);
    let streaming = signal(cx, || true);
    let refresh = signal(cx, || 0usize);
    let usage_open = signal(cx, || false);
    let settings_open = signal(cx, || false);
    let csrf = state.csrf.clone();
    let submit = attributes! {
        cx =>
        @submit=$(async |event: Event| {
            event.prevent_default();
            let prompt = draft.get();
            if !busy.get() {
                if !generating.get() {
                    if !session_archived.get() {
                        if !prompt.trim().is_empty() {
                            busy.set(true);
                            let started = begin_chat(csrf.clone(), session.get(), prompt).await;
                            if started {
                                draft.set("".to_owned());
                                generating.set(true);
                                refresh.increment();
                                let _sent = send_chat(
                                    csrf.clone(),
                                    session.get(),
                                    streaming.get(),
                                ).await;
                            } else {
                                refresh.increment();
                            }
                            busy.set(false);
                        }
                    }
                }
            }
        })
    };
    let reset_csrf = state.csrf.clone();
    Ok(view! {
        <section
            :data-chat-session=$(session.get())
            data-chat-resume=(if resume { "true" } else { "false" })
            @chatresume=$(|_event: Event| {
                refresh.increment();
            })
            class="chat-workspace flex h-[calc(100dvh-80px)] min-h-[560px] w-full overflow-hidden rounded-xl border border-border bg-white shadow-[0_12px_32px_-24px_rgba(16,24,40,.28)] max-[760px]:h-[calc(100dvh-180px)] max-[760px]:min-h-[650px] max-[760px]:flex-col"
            aria-label="模型聊天"
        >
            <aside
                class="flex w-[232px] shrink-0 flex-col border-r border-[#e9edf2] bg-[#f9fafc] px-3 py-4 max-[760px]:w-full max-[760px]:border-r-0 max-[760px]:border-b max-[760px]:py-3"
                aria-label="聊天设置与历史会话"
            >
                <div class="px-2 pb-4 max-[760px]:pb-2">
                    <p
                        class="m-0 text-[11px] font-semibold tracking-[0.13em] text-[#8a94a3]"
                    >
                        "LLMPROXY / CHAT"
                    </p>
                    <h2
                        class="mt-1 mb-0 text-[17px] font-semibold tracking-tight text-heading"
                    >
                        "对话工作区"
                    </h2>
                </div>
                <div class="mb-4 px-2 [&_form]:grid [&_form]:grid-cols-[minmax(0,1fr)_auto] [&_label]:col-span-2 [&_select]:w-full! [&_select]:min-w-0">
                    super::groups::group_selector(scope: "chat")
                </div>
                if available {
                    <button
                        type="button"
                        class="chat-new-button mb-5 flex h-9 w-full items-center justify-center gap-2 rounded-lg border border-[#dce4ee] bg-white text-[13px] font-medium text-heading hover:border-[#a9bdd8] hover:bg-[#f7faff] max-[760px]:mb-3"
                        :disabled=$(busy.get())
                        @click=$(async |_event: Event| {
                            busy.set(true);
                            let next = new_chat(reset_csrf.clone(), session.get()).await;
                            let saved = raw!(
                                "(() => { const payload = ${drafts}.get().dehydrate(); const entries = new Map(payload.v); entries.set(${session}.get().dehydrate(), ${draft}.get().dehydrate()); return cx.hydrate({ ...payload, v: [...entries] }); })()",
                                Vec::<(String, String)>::new(),
                            );
                            drafts.set(saved);
                            session.set(next);
                            generating.set(false);
                            draft.set("".to_owned());
                            selection_error.set("".to_owned());
                            session_archived.set(false);
                            archived_only.set(false);
                            history_page.set(0);
                            refresh.increment();
                            busy.set(false);
                        })
                    >
                        icon(
                            data: PLUS_OUTLINED,
                            attrs: attributes! { class="size-3.5" aria-hidden="true" }
                        )
                        "新会话"
                    </button>
                    <fieldset
                        class="m-0 min-w-0 border-0 p-0"
                        :disabled=$(if session_archived.get() {
                            true
                        } else {
                            generating.get()
                        })
                    >
                        <details
                            class="chat-settings border-t border-[#e9edf2] px-2 pt-4 max-[760px]:pt-3"
                            :open=$(settings_open.get())
                        >
                            <summary
                                class="flex cursor-pointer items-center gap-2 text-[12px] font-semibold text-[#8793a2]"
                                @click=$(|event: Event| {
                                    event.prevent_default();
                                    settings_open.set(!settings_open.get());
                                })
                            >
                                <span
                                    class="chat-settings-caret text-[16px] leading-none"
                                    aria-hidden="true"
                                >
                                    "›"
                                </span>
                                "设置"
                                <span
                                    class="ml-auto flex items-center gap-1.5 text-[11px] font-normal text-secondary"
                                >
                                    <span :hidden=$(protocol.get() != "openai_chat")>
                                        "Chat"
                                    </span>
                                    <span :hidden=$(protocol.get() != "openai_responses")>
                                        "Responses"
                                    </span>
                                    <span :hidden=$(protocol.get() != "anthropic_messages")>
                                        "Messages"
                                    </span>
                                    <span :hidden=$(protocol.get() != "gemini")>"Gemini"</span>
                                    <span aria-hidden="true">"·"</span>
                                    <span :hidden=$(!streaming.get())>"流式"</span>
                                    <span :hidden=$(streaming.get())>"非流式"</span>
                                </span>
                            </summary>
                            <div
                                class="mt-4 grid grid-cols-[64px_minmax(0,1fr)] items-center gap-2"
                            >
                                <label
                                    class="text-[12px] font-medium text-secondary"
                                    for="chat-protocol"
                                >
                                    "协议"
                                </label>
                                chat_protocol_picker(
                                    model_id: $(model_id),
                                    protocol: $(protocol),
                                    session: $(session),
                                    refresh: $(refresh),
                                    busy: $(busy),
                                    selection_error: $(selection_error)
                                )
                            </div>
                            <p
                                class="mt-2 mb-0 text-[11px] leading-[1.5] text-[#8a94a3]"
                            >
                                "切换模型或协议会保留当前对话上下文"
                            </p>
                            chat_thinking_picker(
                                model_id: $(model_id),
                                session: $(session),
                                refresh: $(refresh),
                                busy: $(busy),
                                selection_error: $(selection_error)
                            )
                            <label
                                class="mt-3 grid grid-cols-[64px_minmax(0,1fr)] items-center gap-2 text-[12px] font-medium text-secondary"
                            >
                                <span>"流式输出"</span>
                                <input
                                    type="checkbox"
                                    :checked=$(streaming.get())
                                    :disabled=$(busy.get())
                                    @change=$(|event: Event| {
                                        streaming.set(event.target.checked);
                                    })
                                >
                            </label>
                            <p class="mt-2 mb-0 text-[11px] text-secondary">
                                "流式输出会逐步显示回复。"
                            </p>
                        </details>
                    </fieldset>
                }
                if !selection_error.get().is_empty() {
                    <p role="alert" class="mt-2 mb-0 px-2 text-[12px] text-red-600">
                        $(selection_error.get())
                    </p>
                }
                <div
                    class="mt-6 flex min-h-0 flex-1 flex-col border-t border-[#e9edf2] px-2 pt-4 max-[760px]:mt-3 max-[760px]:pt-3"
                >
                    <div class="flex items-center justify-between">
                        <h3
                            class="m-0 text-[11px] font-semibold tracking-[0.12em] text-[#8793a2]"
                        >
                            $(if archived_only.get() {
                                "已归档"
                            } else {
                                "历史会话"
                            })
                        </h3>
                        <button
                            class="border-0 bg-transparent p-0 text-[11px] text-[#8793a2] hover:text-primary"
                            type="button"
                            :disabled=$(busy.get())
                            @click=$(|_event: Event| {
                                archived_only.set(!archived_only.get());
                                history_page.set(0);
                            })
                        >
                            $(if archived_only.get() {
                                "返回历史"
                            } else {
                                "查看归档"
                            })
                        </button>
                    </div>
                    <div class="mt-2 min-h-0 overflow-y-auto max-[760px]:max-h-24">
                        chat_session_list(
                            page: $(history_page),
                            session: $(session),
                            model_id: $(model_id),
                            protocol: $(protocol),
                            draft: $(draft),
                            drafts: $(drafts),
                            refresh: $(refresh),
                            busy: $(busy),
                            generating: $(generating),
                            archived_only: $(archived_only),
                            session_archived: $(session_archived),
                            error: $(selection_error)
                        )
                    </div>
                </div>
                <p
                    class="mt-3 mb-0 px-2 text-[11px] leading-[1.5] text-[#9aa4b0] max-[760px]:hidden"
                >
                    "会话保存到本地数据库，可刷新后继续"
                </p>
            </aside>
            <div class="flex min-h-0 min-w-0 flex-1 flex-col">
                <header
                    class="relative z-20 flex h-14 shrink-0 items-center justify-between gap-3 border-b border-[#eef0f3] px-7 max-[640px]:px-4"
                >
                    <div class="shrink-0">
                        <h1 class="sr-only">"Chat"</h1>
                        <p class="m-0 text-[11px] text-muted max-[640px]:hidden">
                            "通过网关与已接入模型对话"
                        </p>
                    </div>
                    chat_usage(
                        session: $(session),
                        refresh: $(refresh),
                        open: $(usage_open)
                    )
                </header>
                if available {
                    <div
                        id="chat-scroll"
                        class="flex min-h-0 flex-1 flex-col-reverse overflow-y-auto px-8 py-8 max-[640px]:px-4 max-[640px]:py-5"
                    >
                        <div
                            class="mx-auto flex min-h-full w-full max-w-[1200px] shrink-0 flex-col"
                        >
                            chat_history(session: $(session), refresh: $(refresh))
                        </div>
                    </div>
                    <div class="bg-white px-7 pt-3 pb-3 max-[640px]:px-4">
                        <div class="mx-auto max-w-[1200px]">
                            <p
                                class="mt-0 mb-2 text-[12px] text-secondary"
                                role="status"
                                :hidden=$(!session_archived.get())
                            >
                                "此会话已归档，请在左侧菜单中恢复后继续。"
                            </p>
                            chat_health_updates(refresh: $(refresh))
                            chat_health_notice(
                                selected: $(model_id.get()),
                                current: $(protocol.get()),
                                revision: $(refresh.get()),
                                refresh: $(refresh),
                                busy: $(busy)
                            )
                            <fieldset
                                class="m-0 min-w-0 border-0 p-0"
                                :disabled=$(session_archived.get())
                            >
                                chat_sender(
                                    id: "chat-draft",
                                    draft: &draft,
                                    submit_attrs: submit,
                                    busy: Some(&generating),
                                    max_length: Some(4000),
                                    language: UiLanguage::ChineseSimplified,
                                    attrs: attributes! { class="chat-composer" },
                                    <div class="chat-composer-model flex min-w-0 items-center">
                                        chat_model_picker(
                                            selected: $(model_id.get()),
                                            current: $(protocol.get()),
                                            revision: $(refresh.get()),
                                            model_id: $(model_id),
                                            protocol: $(protocol),
                                            session: $(session),
                                            refresh: $(refresh),
                                            busy: $(busy),
                                            generating: $(generating),
                                            selection_error: $(selection_error)
                                        )
                                    </div>
                                    chat_stop_button(
                                        session: $(session),
                                        refresh: $(refresh),
                                        generating: $(generating)
                                    )
                                )
                            </fieldset>
                        </div>
                    </div>
                } else {
                    <div
                        class="flex flex-1 flex-col items-center justify-center px-6 text-center"
                    >
                        <h2 class="m-0 text-lg font-medium text-heading">
                            "暂无可聊天的模型"
                        </h2>
                        <p class="mt-2 mb-5 text-sm text-secondary">
                            "先在 Models 中添加模型，并启用对应 Provider。"
                        </p>
                        <a
                            class="inline-flex h-9 items-center rounded-md bg-primary px-4 text-sm font-medium text-white hover:bg-primary-hover"
                            (topcoat::runtime::link_attrs(
                                cx,
                                href!(super::models::models),
                                topcoat::runtime::prefetch_mode(cx),
                            ))
                        >
                            "前往 Models"
                        </a>
                    </div>
                    <div class="overflow-y-auto p-6">
                        chat_history(session: $(session), refresh: $(refresh))
                    </div>
                }
            </div>
        </section>
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/new-chat")]
pub async fn new_chat(cx: &Cx, csrf: String, current_session: String) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let sessions = &state.chat_sessions;
    let current = sessions
        .get(&current_session)
        .ok_or_else(|| io::Error::other("聊天会话已过期，请刷新页面"))?;
    let selection = current.selection();
    let protocol_kind = selection.protocol;
    let (_, protocols, available, _) =
        chat_target(&crate::app::group_store(cx, "chat"), &selection.model_id)
            .await
            .map_err(io::Error::other)?;
    if !available || !protocols.contains(&protocol_kind) {
        return Err(io::Error::other("当前模型或协议已不可用").into());
    }
    ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions,
    }
    .create(selection)
    .await
}

/// 校验目标后在原会话内更新下一轮选择，失败时不改变选择及历史。
#[procedure("/ui/_topcoat/runtime/procedures/switch-chat")]
pub async fn switch_chat(
    cx: &Cx,
    csrf: String,
    session_id: String,
    model_id: String,
    protocol: String,
) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    crate::app::group_store(cx, "chat")
        .get_chat_conversation(&session_id)
        .await?;
    let Some(session) = state.chat_sessions.get(&session_id) else {
        return Ok("聊天会话已过期，请刷新页面".into());
    };
    let kind = match selected_protocol(&protocol) {
        Ok(kind) => kind,
        Err(error) => return Ok(error),
    };
    let (_, protocols, available, _) =
        match chat_target(&crate::app::group_store(cx, "chat"), &model_id).await {
            Ok(target) => target,
            Err(error) => return Ok(error),
        };
    if !available || !protocols.contains(&kind) {
        return Ok("所选模型或协议已不可用，当前选择已保留".into());
    }
    let health = crate::app::chat_service::target::health(
        &crate::app::group_store(cx, "chat"),
        &model_id,
        kind,
    )
    .await
    .map_err(io::Error::other)?;
    if health.blocked {
        return Ok(format!(
            "{}，当前选择已保留，请切换协议或模型",
            health.label
        ));
    }
    match (ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    })
    .select(
        &session_id,
        Selection {
            model_id,
            protocol: kind,
        },
        session.thinking_choice(),
    )
    .await
    {
        Ok(()) => Ok(String::new()),
        Err(error) => Ok(error.to_string()),
    }
}

#[procedure("/ui/_topcoat/runtime/procedures/default-chat-protocol")]
pub async fn default_protocol(
    cx: &Cx,
    csrf: String,
    model_id: String,
    current: String,
) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    Ok(crate::app::chat_service::target::choose_protocol(
        &crate::app::group_store(cx, "chat"),
        &model_id,
        &current,
    )
    .await
    .unwrap_or_default())
}

#[procedure("/ui/_topcoat/runtime/procedures/begin-chat")]
pub async fn begin_chat(cx: &Cx, csrf: String, session_id: String, prompt: String) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let service = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    };
    let session = service.load(&session_id).await?;
    let (alias, _, available, _) = match chat_target(
        &crate::app::group_store(cx, "chat"),
        &session.selection().model_id,
    )
    .await
    {
        Ok(target) => target,
        Err(_) => {
            session.save_error("当前模型配置已失效，请选择有效模型");
            return Ok(false);
        }
    };
    if !available {
        session.save_error("当前模型已不可用，请选择有效模型");
        return Ok(false);
    }
    let selection = session.selection();
    let health = crate::app::chat_service::target::health(
        &crate::app::group_store(cx, "chat"),
        &selection.model_id,
        selection.protocol,
    )
    .await
    .map_err(io::Error::other)?;
    if health.blocked {
        session.save_error(&format!(
            "{}；当前选择已保留，请重新探测或切换模型",
            health.label
        ));
        return Ok(false);
    }
    match service.begin(&session_id, &prompt, &alias).await {
        Ok(started) => Ok(started),
        Err(error) => {
            session.save_error(&error.to_string());
            Ok(false)
        }
    }
}

/// 停止控件由服务端活跃状态驱动，刷新页面后仍可取消原请求。
#[shard("/ui/_topcoat/runtime/shards/chat-stop-button")]
pub async fn chat_stop_button(
    cx: &Cx,
    session: Signal<String>,
    refresh: Signal<usize>,
    generating: Signal<bool>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _revision = refresh.get();
    let state = app_context::<AppState>(cx);
    let room_id = session.get();
    let room = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    }
    .load(&room_id)
    .await?;
    let csrf = state.csrf.clone();
    Ok(live! {
        let mut changed = room.subscribe();
        loop {
            let busy = room.snapshot().2;
            let token = emit! {
                // SSR 只输出快照，状态写入在浏览器微任务中执行；旧订阅不能覆盖新选择。
                <span
                    hidden=""
                    :data-chat-generating=$(raw!(
                        "(() => { if (${session}.get().dehydrate() === ${room_id}.dehydrate()) queueMicrotask(() => { if (${session}.get().dehydrate() === ${room_id}.dehydrate()) ${generating}.set(cx.hydrate(${busy}.dehydrate())); }); return ${busy}.dehydrate(); })()",
                        {
                            let _current = session.get();
                            busy
                        },
                    ))
                ></span>
                if busy {
                    <button
                        type="button"
                        class="gr-chat-send-button chat-stop-button"
                        aria-label="停止生成"
                        @click=$(async |_event: Event| {
                            let _stopped = stop_chat(csrf.clone(), session.get()).await;
                        })
                    >
                        <span class="sr-only">"停止生成"</span>
                    </button>
                }
            }?;
            // GET 首屏立即完成；浏览器 POST shard 通过共享连接或 HTTP 等待广播。
            if topcoat::router::request::original_method(cx)
                    != topcoat::router::Method::POST
                || !busy {
                break Ok(token);
            }
            changed.recv().await.ok();
        }
    })
}

/// 停止当前对话请求，不把停止说明写进下一轮发送给模型的历史。
#[procedure("/ui/_topcoat/runtime/procedures/stop-chat")]
pub async fn stop_chat(cx: &Cx, csrf: String, session_id: String) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let service = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    };
    let Ok(room) = service.load(&session_id).await else {
        return Ok(false);
    };
    let stopped = room.stop();
    if stopped && !room.request_active() {
        service
            .finish(&session_id, Err("已停止生成".into()))
            .await?;
    }
    Ok(stopped)
}

/// 提交后台生成任务，HTTP 返回表示已提交而非整轮已经完成。
#[procedure("/ui/_topcoat/runtime/procedures/send-chat")]
pub async fn send_chat(cx: &Cx, csrf: String, session_id: String, streaming: bool) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    crate::app::group_store(cx, "chat")
        .get_chat_conversation(&session_id)
        .await?;
    let Some(session) = state.chat_sessions.get(&session_id) else {
        return Ok(false);
    };
    let Some(turn) = session.start_request() else {
        return Ok(false);
    };
    crate::app::chat_service::generation::spawn(
        state,
        crate::app::group_store(cx, "chat"),
        session_id,
        session,
        turn,
        streaming,
    );
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
    selection_error: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let state = app_context::<AppState>(cx);
    let csrf = state.csrf.clone();
    let (_, protocols, _, _) = chat_target(&crate::app::group_store(cx, "chat"), &model_id.get())
        .await
        .unwrap_or_else(|_| (String::new(), Vec::new(), false, Support::Unknown));
    let mut options = Vec::new();
    for kind in protocols {
        let health = crate::app::chat_service::target::health(
            &crate::app::group_store(cx, "chat"),
            &model_id.get(),
            kind,
        )
        .await
        .map_err(io::Error::other)?;
        options.push((kind, health));
    }
    Ok(view! {
        select(
            attrs: attributes! {
                cx =>
                id="chat-protocol"
                aria-label="选择模型协议"
                class="w-full"
                :value=$(protocol.get())
                :disabled=$(busy.get())
                @change=$(async |event: Event| {
                    busy.set(true);
                    let next_protocol = event.target.value;
                    let error = switch_chat(
                        csrf.clone(),
                        session.get(),
                        model_id.get(),
                        next_protocol.clone(),
                    ).await;
                    if error.is_empty() {
                        protocol.set(next_protocol);
                        refresh.increment();
                    } else {
                        protocol.set(protocol.get().clone());
                    }
                    selection_error.set(error);
                    busy.set(false);
                })
            },
            for (kind, health) in &options {
                <option value=(kind.as_str()) :disabled=(health.blocked)>
                    (protocol_label(*kind))
                </option>
            }
        )
    })
}

/// 页面信号只在服务端组件之间共享，shard 的传输参数仍保持独立 Signal。
struct HistoryControls {
    page: Signal<usize>,
    session: Signal<String>,
    model_id: Signal<String>,
    protocol: Signal<String>,
    draft: Signal<String>,
    drafts: Signal<Vec<(String, String)>>,
    refresh: Signal<usize>,
    busy: Signal<bool>,
    generating: Signal<bool>,
    archived_only: Signal<bool>,
    session_archived: Signal<bool>,
    error: Signal<String>,
}

#[shard("/ui/_topcoat/runtime/shards/chat-session-list")]
pub async fn chat_session_list(
    cx: &Cx,
    page: Signal<usize>,
    session: Signal<String>,
    model_id: Signal<String>,
    protocol: Signal<String>,
    draft: Signal<String>,
    drafts: Signal<Vec<(String, String)>>,
    refresh: Signal<usize>,
    busy: Signal<bool>,
    generating: Signal<bool>,
    archived_only: Signal<bool>,
    session_archived: Signal<bool>,
    error: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _revision = refresh.get();
    let mut aliases: HashMap<_, _> = crate::app::group_store(cx, "chat")
        .list_models()
        .await?
        .into_iter()
        .map(|model| (model.id.to_string(), model.alias))
        .collect();
    for route in crate::app::group_store(cx, "chat").list_routes().await? {
        aliases.insert(format!("route:{}", route.id), route.name);
    }
    let records = if archived_only.get() {
        crate::app::group_store(cx, "chat")
            .list_archived_chat_conversations(page.get() * 20, 21)
            .await?
    } else {
        crate::app::group_store(cx, "chat")
            .list_chat_conversations(page.get() * 20, 21)
            .await?
    };
    let has_next = records.len() > 20;
    let rooms: Vec<_> = records
        .into_iter()
        .take(20)
        .map(|record| {
            let alias = aliases
                .get(&record.selection.model_id)
                .cloned()
                .unwrap_or_else(|| "模型配置已失效".into());
            (record, alias)
        })
        .collect();
    let controls = HistoryControls {
        page: page.clone(),
        session,
        model_id,
        protocol,
        draft,
        drafts,
        refresh,
        busy: busy.clone(),
        generating,
        archived_only: archived_only.clone(),
        session_archived,
        error,
    };
    Ok(view! {
        <nav
            class="grid gap-1"
            aria-label=(if controls.archived_only.get() {
                "已归档历史会话"
            } else {
                "已保存历史会话"
            })
        >
            if rooms.is_empty() {
                <p class="my-2 text-[11px] text-secondary">
                    (if controls.archived_only.get() {
                        "暂无归档会话"
                    } else {
                        "暂无历史会话"
                    })
                </p>
            }
            #[key(record.id.clone())]
            for (record, alias) in rooms {
                chat_session_item(record: record, alias: alias, controls: &controls)
            }
            <div class="mt-2 flex justify-between text-[11px] text-secondary">
                if page.get() > 0 {
                    <button
                        type="button"
                        :disabled=$(busy.get())
                        @click=$(|_event: Event| {
                            page.set(page.get() - 1);
                        })
                    >
                        "上一页"
                    </button>
                }
                if has_next {
                    <button
                        type="button"
                        :disabled=$(busy.get())
                        @click=$(|_event: Event| {
                            page.increment();
                        })
                    >
                        "下一页"
                    </button>
                }
            </div>
        </nav>
    })
}

/// 会话选择与菜单是同一行的两个按钮，菜单点击不会触发切换会话。
#[component]
async fn chat_session_item(
    cx: &Cx,
    record: llmproxy_store::chat_history::Conversation,
    alias: String,
    controls: &HistoryControls,
) -> Result<impl View> {
    let title = if record.title.is_empty() {
        "新会话".to_owned()
    } else {
        record.title.clone()
    };
    let id = record.id.clone();
    let model = record.selection.model_id.clone();
    let kind = record.selection.protocol.as_str().to_owned();
    let archived = record.archived;
    let session = controls.session.clone();
    let model_id = controls.model_id.clone();
    let protocol = controls.protocol.clone();
    let draft = controls.draft.clone();
    let drafts = controls.drafts.clone();
    let refresh = controls.refresh.clone();
    let busy = controls.busy.clone();
    let generating = controls.generating.clone();
    let item_running = signal(cx, || record.active_turn.is_some());
    let session_archived = controls.session_archived.clone();
    let error = controls.error.clone();
    let csrf = app_context::<AppState>(cx).csrf.clone();
    let unavailable = "无法打开会话，请刷新后重试".to_owned();
    let menu_id = format!("chat-actions-{id}");
    let menu_label = format!("会话「{title}」的操作");
    let dialog_id = format!("chat-delete-{id}");
    let archive_id = format!("chat-archive-{id}");
    let archive_description = format!("归档「{title}」后只能查看内容与用量，恢复后才能继续对话。");
    let archive_anchor_attrs = attributes! {
        if !archived {
            aria-controls=(archive_id.as_str())
            style=(format!("anchor-name: --gr-{archive_id}"))
        }
    };
    let confirm_open = signal(cx, || false);
    let archive_attrs = history_action_attributes(cx, controls, &id, "archive", &confirm_open);
    let restore_attrs = history_action_attributes(cx, controls, &id, "restore", &confirm_open);
    let delete_attrs = history_action_attributes(cx, controls, &id, "delete", &confirm_open);
    Ok(view! {
        // 气泡锚定始终可见的会话行，避免操作菜单关闭后丢失定位。
        <div
            class="chat-session-item flex min-w-0 items-center rounded-lg hover:bg-[#edf2f8]"
            :data-active=$(session.get() == id)
            data-archived=(if archived { "true" } else { "false" })
            (archive_anchor_attrs)
        >
            <button
                type="button"
                class="min-w-0 flex-1 rounded-lg border-0 bg-transparent px-2.5 py-2 text-left text-inherit"
                :disabled=$(busy.get())
                @click=$(async |_event: Event| {
                    busy.set(true);
                    let result = raw!(
                        "await Promise.resolve(${open_chat}.call(${csrf}, ${id})).catch(() => ${unavailable})",
                        unavailable.clone(),
                    );
                    if result.is_empty() {
                        let saved = raw!(
                            "(() => { const payload = ${drafts}.get().dehydrate(); const entries = new Map(payload.v); entries.set(${session}.get().dehydrate(), ${draft}.get().dehydrate()); return cx.hydrate({ ...payload, v: [...entries] }); })()",
                            Vec::<(String, String)>::new(),
                        );
                        drafts.set(saved);
                        session.set(id.clone());
                        generating.set(item_running.get());
                        model_id.set(model.clone());
                        protocol.set(kind.clone());
                        session_archived.set(archived);
                        let next_draft = raw!(
                            "cx.hydrate(new Map(${drafts}.get().dehydrate().v).get(${id}.dehydrate()) ?? '')",
                            String::new(),
                        );
                        draft.set(next_draft);
                    }
                    error.set(result);
                    busy.set(false);
                })
            >
                <span
                    class="flex min-w-0 items-center gap-1.5 text-[12px] font-medium leading-5"
                >
                    <span class="truncate">(title.clone())</span>
                    chat_session_activity(
                        id: $(id.clone()),
                        running: $(item_running),
                        refresh: $(refresh)
                    )
                </span>
                <span
                    class="chat-session-model mt-0.5 block truncate text-[11px] text-[#8a94a3]"
                >
                    (format!(
                        "{} · {}",
                        alias,
                        protocol_label(record.selection.protocol),
                    ))
                </span>
            </button>
            <button
                class="mr-1 flex size-7 shrink-0 items-center justify-center rounded-md border-0 bg-transparent p-0 text-[20px] leading-none text-secondary hover:bg-[#dce7f5] hover:text-heading"
                type="button"
                aria-label=(format!("更多操作：{title}"))
                title="会话操作"
                :disabled=$(if busy.get() { true } else { item_running.get() })
                (anchored_menu_trigger_attributes(cx, &menu_id))
            >
                <span aria-hidden="true">"…"</span>
            </button>
        </div>
        anchored_menu(
            id: menu_id.as_str(),
            label: menu_label.as_str(),
            attrs: attributes! { class="min-w-[128px]!" },
            if archived {
                <button
                    type="button"
                    class="block w-full rounded-md border-0 bg-transparent px-3 py-2 text-left text-[12px] text-heading hover:bg-[#edf2f8]"
                    :disabled=$(busy.get())
                    (restore_attrs)
                >
                    "恢复"
                </button>
            } else {
                <button
                    type="button"
                    class="block w-full rounded-md border-0 bg-transparent px-3 py-2 text-left text-[12px] text-heading hover:bg-[#edf2f8]"
                    :disabled=$(busy.get())
                    popovertarget=(archive_id.as_str())
                    popovertargetaction="show"
                    aria-haspopup="dialog"
                    aria-controls=(archive_id.as_str())
                >
                    "归档"
                </button>
            }
            <button
                type="button"
                class="block w-full rounded-md border-0 bg-transparent px-3 py-2 text-left text-[12px] text-[#cf1322]! hover:bg-[#fff2f0]"
                :disabled=$(busy.get())
                @click=$(|_event: Event| {
                    confirm_open.set(true);
                })
            >
                "删除"
            </button>
        )
        if !archived {
            popconfirm(
                id: archive_id.as_str(),
                title: "确认归档会话？",
                description: Some(archive_description.as_str()),
                language: UiLanguage::ChineseSimplified,
                attrs: attributes! { class="chat-archive-confirm" },
                <button
                    type="button"
                    class="gr-button gr-button-primary"
                    :disabled=$(busy.get())
                    (archive_attrs)
                >
                    "确认归档"
                </button>
            )
        }
        native_dialog(
            config: NativeDialogConfig::new(&dialog_id, "删除会话？"),
            busy: &busy,
            open: Some(&confirm_open),
            language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="chat-delete-dialog w-[min(420px,calc(100%_-_32px))]!" },
            <p class="m-0 break-words px-6 py-5 text-[13px] text-secondary">
                (format!("「{title}」的对话内容与用量记录将永久删除。"))
            </p>
            <footer class="flex justify-end gap-2 border-t border-border px-6 py-3">
                <button
                    type="button"
                    class="gr-button gr-button-default"
                    :disabled=$(busy.get())
                    (native_dialog_close_attributes(cx, &dialog_id))
                >
                    "取消"
                </button>
                <button
                    type="button"
                    class="gr-button gr-button-danger"
                    :disabled=$(busy.get())
                    (delete_attrs)
                >
                    "确认删除"
                </button>
            </footer>
        )
    })
}

/// 每条会话单独订阅生成状态，后台结束后更新标记和该行操作按钮。
#[shard("/ui/_topcoat/runtime/shards/chat-session-activity")]
pub async fn chat_session_activity(
    cx: &Cx,
    id: String,
    running: Signal<bool>,
    refresh: Signal<usize>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _revision = refresh.get();
    crate::app::group_store(cx, "chat")
        .get_chat_conversation(&id)
        .await?;
    let room = app_context::<AppState>(cx).chat_sessions.get(&id);
    Ok(live! {
        let mut changed = room.as_ref().map(|room| room.subscribe());
        loop {
            let active = room.as_ref().is_some_and(|room| room.snapshot().2);
            let token = emit! {
                <span
                    class="shrink-0 text-[10px] font-normal text-primary"
                    :data-chat-activity=$(raw!(
                        "(() => { if (${running}.get().dehydrate() !== ${active}.dehydrate()) queueMicrotask(() => ${running}.set(cx.hydrate(${active}.dehydrate()))); return ${active}.dehydrate(); })()",
                        {
                            let _previous = running.get_untracked();
                            active
                        },
                    ))
                >
                    if active {
                        "生成中"
                    }
                </span>
            }?;
            if topcoat::router::request::original_method(cx)
                    != topcoat::router::Method::POST
                || !active {
                break Ok(token);
            }
            if let Some(changed) = changed.as_mut() {
                changed.recv().await.ok();
            }
        }
    })
}

#[topcoat::runtime::record]
#[derive(Clone, Debug)]
pub struct ChatSelectionRecord {
    pub session_id: String,
    pub model_id: String,
    pub protocol: String,
}

type HistoryOutcome = std::result::Result<Option<ChatSelectionRecord>, String>;

/// 菜单和删除确认复用同一操作流程，失败时保留当前会话和草稿并解除忙碌状态。
fn history_action_attributes(
    cx: &Cx,
    controls: &HistoryControls,
    id: &str,
    action: &str,
    confirm_open: &Signal<bool>,
) -> Attributes {
    let session = controls.session.clone();
    let model_id = controls.model_id.clone();
    let protocol = controls.protocol.clone();
    let draft = controls.draft.clone();
    let refresh = controls.refresh.clone();
    let busy = controls.busy.clone();
    let generating = controls.generating.clone();
    let page = controls.page.clone();
    let archived_only = controls.archived_only.clone();
    let session_archived = controls.session_archived.clone();
    let error = controls.error.clone();
    let csrf = app_context::<AppState>(cx).csrf.clone();
    let id = id.to_owned();
    let action = action.to_owned();
    let unavailable: HistoryOutcome = Err("会话操作失败，请刷新后重试".into());
    attributes! {
        cx =>
        @click=$(async |_event: Event| {
            busy.set(true);
            let result = raw!(
                "await Promise.resolve(${change_chat_history}.call(${csrf}, ${session}.get(), ${id}, ${action})).catch(() => ${unavailable})",
                unavailable.clone(),
            );
            if result.is_ok() {
                let next = result.unwrap();
                if next.is_some() {
                    let next = next.unwrap();
                    session.set(next.session_id);
                    generating.set(false);
                    model_id.set(next.model_id);
                    protocol.set(next.protocol);
                    draft.set("".to_owned());
                    session_archived.set(false);
                    archived_only.set(false);
                } else if session.get() == id {
                    if action == "restore" {
                        session_archived.set(false);
                        archived_only.set(false);
                    }
                }
                page.set(0);
                error.set("".to_owned());
                confirm_open.set(false);
                refresh.increment();
            } else {
                error.set(result.unwrap_err());
            }
            busy.set(false);
        })
    }
}

/// 点击旧记录时先恢复类型化 IR，页面信号随后切换。
#[procedure("/ui/_topcoat/runtime/procedures/open-chat")]
pub async fn open_chat(cx: &Cx, csrf: String, session_id: String) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    match (ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    })
    .load(&session_id)
    .await
    {
        Ok(_) => Ok(String::new()),
        Err(error) => Ok(error.to_string()),
    }
}
/// 服务端校验目标操作；只在当前会话离开历史列表时返回新的工作区选择。
#[procedure("/ui/_topcoat/runtime/procedures/change-chat-history")]
pub async fn change_chat_history(
    cx: &Cx,
    csrf: String,
    current: String,
    target: String,
    action: String,
) -> Result<HistoryOutcome> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let service = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    };
    let action = match action.as_str() {
        "archive" => HistoryAction::Archive,
        "restore" => HistoryAction::Restore,
        "delete" => HistoryAction::Delete,
        _ => return Ok(Err("无效的会话操作".into())),
    };
    Ok(service
        .change_history(&current, &target, action)
        .await
        .map(|next| {
            next.map(|(id, selection)| ChatSelectionRecord {
                session_id: id,
                model_id: selection.model_id,
                protocol: selection.protocol.as_str().to_owned(),
            })
        })
        .map_err(|error| error.to_string()))
}

#[shard("/ui/_topcoat/runtime/shards/chat-history")]
pub async fn chat_history(
    cx: &Cx,
    session: Signal<String>,
    refresh: Signal<usize>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _revision = refresh.get();
    let state = app_context::<AppState>(cx);
    let service = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    };
    let room = service.load(&session.get()).await?;
    let csrf = state.csrf.clone();
    Ok(live! {
        let mut changed = room.subscribe();
        let mut deadline = None;
        loop {
            let mut pending = false;
            if !room.snapshot().2 {
                let records = match crate::app::group_store(cx, "chat").chat_turns(&session.get()).await {
                    Ok(records) => records,
                    Err(_) => {
                        room.save_error(
                            "无法读取历史保存状态，请恢复数据库后重试保存",
                        );
                        Vec::new()
                    }
                };
                for record in &records {
                    room.apply_record(record);
                    pending |= record.call_started && !record.call_finished;
                    if crate::app::group_store(cx, "chat").chat_usage_pending(&record.id) {
                        room.save_error(
                            "本轮来源用量尚未保存，请重试保存",
                        );
                    }
                }
            }
            let save_error = room.persistence_error();
            let (messages, thinking, busy) = room.snapshot();
            let usage = room.usage_snapshot();
            let parts = room.parts_snapshot();
            let models = room.model_snapshot();
            let compact = room.compact_snapshot();
            let warnings = room.warning_snapshot();
            let entries: Vec<_> = messages
                .into_iter()
                .map(
                    |message| {
                        let thought = thinking
                            .get(&message.id)
                            .cloned()
                            .unwrap_or_default();
                        let usage = usage.get(&message.id).cloned().unwrap_or_default();
                        let parts = parts.get(&message.id).cloned().unwrap_or_default();
                        let model = models.get(&message.id).cloned().unwrap_or_default();
                        let compact = compact
                            .get(&message.id)
                            .cloned()
                            .unwrap_or_default();
                        let warning = warnings
                            .get(&message.id)
                            .cloned()
                            .unwrap_or_default();
                        (message, thought, usage, parts, model, compact, warning)
                    },
                )
                .collect();
            let token = emit! {
                if !save_error.is_empty() {
                    <p role="alert" class="mb-3 text-sm text-red-600">
                        (save_error.as_str())
                    </p>
                    <button
                        type="button"
                        @click=$(async |_event: Event| {
                            let _saved = retry_chat_save(csrf.clone(), session.get()).await;
                            refresh.increment();
                        })
                    >
                        "重试保存"
                    </button>
                }
                if entries.is_empty() {
                    <div class="flex min-h-[300px] flex-1 flex-col justify-center">
                        <h2
                            class="m-0 text-[25px] font-semibold tracking-tight text-heading"
                        >
                            "今天想聊什么？"
                        </h2>
                        <p class="mt-2 mb-0 text-sm leading-6 text-secondary">
                            "从下方选择模型，输入你的问题。历史会话保存到本地数据库。"
                        </p>
                    </div>
                } else {
                    chat_message_list(
                        label: "聊天消息",
                        attrs: attributes! { class="pb-2" },
                        #[key(message.id.clone())]
                        for (message, thought, usage, parts, model, compact, warning) in entries {
                            chat_message_entry(
                                message: message,
                                thought: thought,
                                usage: usage,
                                parts: parts,
                                model: model,
                                compact: compact,
                                warning: warning
                            )
                        }
                    )
                }
            }?;
            // 首屏不能等待整轮结束，否则浏览器尚未绑定停止等事件。
            if topcoat::router::request::original_method(cx)
                != topcoat::router::Method::POST {
                break Ok(token);
            }
            if !busy {
                if pending
                    && tokio::time::Instant::now()
                        < *deadline.get_or_insert_with(
                            || tokio::time::Instant::now()
                                    + std::time::Duration::from_secs(5),
                        ) {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
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
    model: String,
    compact: String,
    warning: String,
) -> Result<impl View> {
    let thought_id = format!("chat-thought-{}", message.id);
    let thought_open = signal(cx, || false);
    let details_open = signal(cx, || false);
    Ok(view! {
        chat_bubble(
            role: message.role,
            status: if message.status == ChatMessageStatus::Complete {
                None
            } else {
                Some(message.status)
            },
            language: UiLanguage::ChineseSimplified,
            if message.role == ChatBubbleRole::Assistant && !thought.is_empty() {
                chat_think(
                    id: thought_id.as_str(),
                    open: &thought_open,
                    language: UiLanguage::ChineseSimplified,
                    attrs: attributes! { class="mb-3" },
                    <p
                        class="m-0 max-h-56 overflow-y-auto whitespace-pre-wrap break-words"
                    >
                        (thought.as_str())
                    </p>
                )
            }
            if message.role == ChatBubbleRole::User
                || message.status != ChatMessageStatus::Complete {
                <p class="m-0 whitespace-pre-wrap break-words">
                    (if message.content.is_empty() {
                        if thought.is_empty() {
                            "正在等待回复…"
                        } else {
                            "正在思考…"
                        }
                    } else {
                        message.content.as_str()
                    })
                </p>
            }
            if message.role == ChatBubbleRole::Assistant {
                if parts.is_empty() {
                    if message.status == ChatMessageStatus::Complete {
                        chat_markdown(source: message.content.as_str())
                    }
                } else {
                    for part in &parts {
                        if part.title.is_empty() {
                            if message.status == ChatMessageStatus::Complete {
                                chat_markdown(source: part.text.as_str())
                            }
                        } else {
                            <section
                                class="my-3 rounded-lg border border-border bg-[#f9fafc] p-3"
                                aria-label=(part.title.as_str())
                            >
                                <h4
                                    class="mt-0 mb-2 text-[12px] font-semibold text-heading"
                                >
                                    (part.title.as_str())
                                </h4>
                                if !part.text.is_empty() {
                                    <pre
                                        class="m-0 max-h-72 overflow-auto whitespace-pre-wrap break-words font-mono text-[12px] leading-5"
                                    >
                                        (part.text.as_str())
                                    </pre>
                                }
                                if let Some(media) = &part.media {
                                    if media.preview {
                                        match media.kind {
                                            llmproxy_core::ir::media::MediaKind::Image => {
                                                <img
                                                    src=(media.uri.as_str())
                                                    alt=(part.title.as_str())
                                                    class="max-h-96 max-w-full rounded-md"
                                                    loading="lazy"
                                                >
                                            }
                                            llmproxy_core::ir::media::MediaKind::Audio => {
                                                <audio
                                                    src=(media.uri.as_str())
                                                    controls=""
                                                    preload="none"
                                                    aria-label=(part.title.as_str())
                                                    class="max-w-full"
                                                ></audio>
                                            }
                                            llmproxy_core::ir::media::MediaKind::Video => {
                                                <video
                                                    src=(media.uri.as_str())
                                                    controls=""
                                                    preload="none"
                                                    aria-label=(part.title.as_str())
                                                    class="max-h-96 max-w-full rounded-md"
                                                ></video>
                                            }
                                            llmproxy_core::ir::media::MediaKind::File => {

                                            }
                                        }
                                    }
                                    <a
                                        href=(media.uri.as_str())
                                        download=(part.title.as_str())
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        class="mt-2 inline-block text-[12px] text-primary"
                                    >
                                        "下载附件"
                                    </a>
                                }
                            </section>
                        }
                    }
                }
            }
            if message.role == ChatBubbleRole::Assistant && !compact.is_empty() {
                <details
                    class="chat-turn-details mt-3 text-[11px] text-secondary"
                    :open=$(details_open.get())
                    aria-label="本轮调用详情"
                >
                    <summary
                        class="flex cursor-pointer items-start gap-1.5 rounded py-1 hover:text-heading"
                        title="in / out / cache 分别为输入、输出、缓存读取 token；— 表示未报告"
                        @click=$(|event: Event| {
                            event.prevent_default();
                            details_open.set(!details_open.get());
                        })
                    >
                        <span
                            class="chat-turn-caret shrink-0 text-[14px] leading-[18px]"
                            aria-hidden="true"
                        >
                            "›"
                        </span>
                        <span class="break-words">(compact.as_str())</span>
                    </summary>
                    <div
                        class="mt-1 rounded-lg border border-border bg-[#f9fafc] px-3 py-2 leading-6"
                    >
                        if !model.is_empty() {
                            <p class="m-0 break-words" aria-label="本轮模型">
                                (model.as_str())
                            </p>
                        }
                        if !usage.is_empty() {
                            <p
                                class="mt-1 mb-0 break-words"
                                aria-label="本轮词元用量"
                            >
                                (usage.as_str())
                            </p>
                        }
                    </div>
                </details>
            }
            if !warning.is_empty() {
                <p
                    class="mt-2 mb-0 text-[11px] text-secondary"
                    aria-label="历史兼容提示"
                >
                    (warning.as_str())
                </p>
            }
        )
    })
}

/// 会话偏好由服务端保存，非法选择和生成中的修改都不会生效。
#[procedure("/ui/_topcoat/runtime/procedures/set-chat-thinking")]
pub async fn set_chat_thinking(
    cx: &Cx,
    csrf: String,
    session_id: String,
    choice: String,
) -> Result<String> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    crate::app::group_store(cx, "chat")
        .get_chat_conversation(&session_id)
        .await?;
    let Some(session) = state.chat_sessions.get(&session_id) else {
        return Ok("聊天会话已过期，请刷新页面".into());
    };
    let Some(choice) = Choice::parse(&choice) else {
        return Ok("思考模式无效".into());
    };
    let selection = session.selection();
    let (_, _, _, support) = chat_target(&crate::app::group_store(cx, "chat"), &selection.model_id)
        .await
        .map_err(io::Error::other)?;
    if let Err(message) = (Config {
        support,
        ..Default::default()
    })
    .check(choice)
    {
        return Ok(message.into());
    }
    if session.selection() != selection {
        return Ok("模型选择已改变，请重试".into());
    }
    match (ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    })
    .select(&session_id, selection, choice)
    .await
    {
        Ok(()) => Ok(String::new()),
        Err(error) => Ok(error.to_string()),
    }
}

/// 能力与偏好分开显示，切换模型后保留不兼容偏好并明确提示，不能静默更改意图。
#[shard("/ui/_topcoat/runtime/shards/chat-thinking-picker")]
pub async fn chat_thinking_picker(
    cx: &Cx,
    model_id: Signal<String>,
    session: Signal<String>,
    refresh: Signal<usize>,
    busy: Signal<bool>,
    selection_error: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _revision = refresh.get();
    let state = app_context::<AppState>(cx);
    let room = state
        .chat_sessions
        .get(&session.get())
        .ok_or_else(|| io::Error::other("聊天会话已过期，请刷新页面"))?;
    let current = room.thinking_choice();
    let (_, _, _, support) = chat_target(&crate::app::group_store(cx, "chat"), &model_id.get())
        .await
        .unwrap_or_else(|_| (String::new(), Vec::new(), false, Support::Unknown));
    let warning = (Config {
        support,
        ..Default::default()
    })
    .check(current)
    .err();
    // 会话或已保存偏好变化后，不能复用浏览器携带的旧控件值。
    let choice = signal(&cx.keyed((session.get(), current.as_str())), || {
        current.as_str().to_owned()
    });
    let csrf = state.csrf.clone();
    Ok(view! {
        <div class="mt-3" data-thinking-support=(support.as_str())>
            if support == Support::Switchable
                || support == Support::AlwaysOn
                || current != Choice::Default {
                <div class="grid grid-cols-[64px_minmax(0,1fr)] items-center gap-2">
                    <label
                        class="text-[12px] font-medium text-secondary"
                        for="chat-thinking"
                    >
                        "思考模式"
                    </label>
                    select(
                        attrs: attributes! {
                            cx =>
                            id="chat-thinking"
                            aria-label="思考模式"
                            class="w-full"
                            :value=$(choice.get())
                            :disabled=$(busy.get())
                            @change=$(async |event: Event| {
                                busy.set(true);
                                let next = event.target.value;
                                let error = set_chat_thinking(
                                    csrf.clone(),
                                    session.get(),
                                    next.clone(),
                                ).await;
                                if error.is_empty() {
                                    choice.set(next);
                                    refresh.increment();
                                } else {
                                    choice.set(choice.get().clone());
                                }
                                selection_error.set(error);
                                busy.set(false);
                            })
                        },
                        <option value="default">"默认"</option>
                        <option
                            value="enabled"
                            disabled=(!matches!(
                                support,
                                Support::Switchable | Support::AlwaysOn,
                            ))
                        >
                            "开启"
                        </option>
                        <option
                            value="disabled"
                            disabled=(support != Support::Switchable)
                        >
                            "关闭"
                        </option>
                    )
                </div>
                if support == Support::AlwaysOn {
                    <p class="mt-1 mb-0 text-[11px] text-secondary">
                        "模型始终开启思考"
                    </p>
                }
                <p class="mt-1 mb-0 text-[11px] text-secondary">
                    "开启时请求模型支持的可见思考或摘要。"
                </p>
                if let Some(message) = warning {
                    <p role="alert" class="mt-1 mb-0 text-[11px] text-red-600">
                        (message)
                    </p>
                }
            } else {
                <div
                    class="grid grid-cols-[64px_minmax(0,1fr)] items-center gap-2 text-[12px] text-secondary"
                >
                    <span class="font-medium">"思考模式"</span>
                    <span>(support.label())</span>
                </div>
            }
        </div>
    })
}

/// 合计标明计数覆盖范围，未报告字段不会补零。
fn counter_label(
    name: &str,
    counter: &llmproxy_store::chat_history::Counter,
    turns: u64,
) -> String {
    match counter.total {
        Some(total) => format!("{name} {total}（{}/{turns} 轮已报告）", counter.reported),
        None => format!("{name} 未报告"),
    }
}
fn totals_label(t: &llmproxy_store::chat_history::Totals) -> String {
    let mut fields = vec![
        format!("{} 轮", t.turns),
        counter_label("输入", &t.input, t.turns),
        counter_label("输出", &t.output, t.turns),
        counter_label("总计", &t.total, t.turns),
        counter_label("缓存读取", &t.read, t.turns),
        counter_label("缓存写入", &t.write, t.turns),
        counter_label("推理", &t.reasoning, t.turns),
    ];
    if let Some(rate) = t.hit_rate() {
        fields.push(format!(
            "缓存命中率 {rate:.1}%（{} 轮可统计）",
            t.hit_reported
        ));
    }
    fields.push(format!(
        "部分用量 {} 轮 · 用量未报告 {} 轮",
        t.partial, t.unreported
    ));
    fields.join(" · ")
}
/// 标题栏独立订阅轮次终态；生成期间展示已保存累计值，避免逐帧查询数据库。
#[shard("/ui/_topcoat/runtime/shards/chat-usage")]
pub async fn chat_usage(
    cx: &Cx,
    session: Signal<String>,
    refresh: Signal<usize>,
    open: Signal<bool>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _revision = refresh.get();
    let state = app_context::<AppState>(cx);
    let room = ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    }
    .load(&session.get())
    .await?;
    let mut records = crate::app::group_store(cx, "chat")
        .chat_turns(&session.get())
        .await
        .unwrap_or_default();
    Ok(live! {
        let mut changed = room.subscribe();
        let mut deadline = None;
        loop {
            let busy = room.snapshot().2;
            if !busy && let Ok(saved) = crate::app::group_store(cx, "chat").chat_turns(&session.get()).await {
                records = saved;
            }
            let pending = records
                .iter()
                .any(|record| record.call_started && !record.call_finished);
            let summary = llmproxy_store::chat_history::Summary::from_turns(&records);
            let total = summary
                .totals
                .total
                .total
                .map(|total| format!("总计 {total} tokens"))
                .unwrap_or_else(
                    || if records.is_empty() {
                        "暂无用量".into()
                    } else {
                        "总计 未报告".into()
                    },
                );
            let rate = summary
                .totals
                .hit_rate()
                .map(|rate| format!("缓存命中 {rate:.1}%"))
                .unwrap_or_else(|| "缓存 未报告".into());
            let token = emit! {
                <div
                    class="relative min-w-0"
                    aria-label="会话用量统计"
                    @keydown=$(|event: Event| {
                        if event.key == "Escape" {
                            open.set(false);
                        }
                    })
                >
                    <button
                        type="button"
                        class="chat-usage-trigger flex min-w-0 flex-col items-end gap-0.5 rounded-lg px-3 py-1.5 text-right hover:bg-[#f3f6fa]"
                        aria-label="会话累计用量"
                        :aria-expanded=$(open.get())
                        aria-controls="chat-usage-panel"
                        @click=$(|_event: Event| {
                            open.set(!open.get());
                        })
                    >
                        <span class="text-[11px] font-semibold text-heading">
                            "会话用量 · "
                            (if busy {
                                "生成中"
                            } else if pending {
                                "待确认"
                            } else {
                                "已保存"
                            })
                        </span>
                        <span class="text-[11px] text-secondary">
                            (total.as_str())
                            <span class="max-[640px]:hidden">
                                " · "
                                (rate.as_str())
                            </span>
                        </span>
                    </button>
                    <div
                        class="fixed inset-0 z-30"
                        :hidden=$(!open.get())
                        aria-hidden="true"
                        @click=$(|_event: Event| {
                            open.set(false);
                        })
                    ></div>
                    <section
                        id="chat-usage-panel"
                        class="absolute top-full right-0 z-40 mt-2 w-[480px] max-w-[calc(100vw-32px)] overflow-hidden rounded-xl border border-[#dce4ee] bg-white text-left shadow-[0_12px_40px_rgba(24,39,68,0.16)]"
                        :hidden=$(!open.get())
                        aria-label="会话用量详情"
                    >
                        <div
                            class="flex items-center justify-between border-b border-border px-4 py-3"
                        >
                            <h2 class="m-0 text-[13px] font-semibold text-heading">
                                "会话累计用量"
                            </h2>
                            <button
                                type="button"
                                class="rounded px-2 py-1 text-[12px] text-secondary hover:bg-surface"
                                aria-label="关闭用量面板"
                                @click=$(|_event: Event| {
                                    open.set(false);
                                })
                            >
                                "关闭"
                            </button>
                        </div>
                        <div
                            class="max-h-[min(60dvh,400px)] overflow-y-auto overscroll-contain px-4 py-3 text-[11px] leading-6 text-secondary"
                        >
                            if busy {
                                <p class="mt-0 mb-2 text-muted">
                                    "当前显示已保存的累计用量，本轮结束后更新。"
                                </p>
                            }
                            <p class="m-0">(totals_label(&summary.totals))</p>
                            for model in &summary.models {
                                <section class="mt-3 border-t border-border pt-3">
                                    <strong class="break-words text-heading">
                                        (format!(
                                            "{} / {}",
                                            model.provider_name,
                                            model.upstream_model,
                                        ))
                                    </strong>
                                    <p class="mt-1 mb-0">(totals_label(&model.totals))</p>
                                </section>
                            }
                        </div>
                    </section>
                </div>
            }?;
            if topcoat::router::request::original_method(cx)
                != topcoat::router::Method::POST {
                break Ok(token);
            }
            if !busy {
                if pending
                    && tokio::time::Instant::now()
                        < *deadline.get_or_insert_with(
                            || tokio::time::Instant::now()
                                    + std::time::Duration::from_secs(5),
                        ) {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                break Ok(token);
            }
            changed.recv().await.ok();
        }
    })
}

/// 重试同一份终态记录，不重新调用 Provider。
#[procedure("/ui/_topcoat/runtime/procedures/retry-chat-save")]
pub async fn retry_chat_save(cx: &Cx, csrf: String, session_id: String) -> Result<bool> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    ChatService {
        store: &crate::app::group_store(cx, "chat"),
        sessions: &state.chat_sessions,
    }
    .retry_save(&session_id)
    .await?;
    Ok(state
        .chat_sessions
        .get(&session_id)
        .is_some_and(|room| room.persistence_error().is_empty()))
}

#[shard("/ui/_topcoat/runtime/shards/chat-model-picker")]
pub async fn chat_model_picker(
    cx: &Cx,
    selected: String,
    current: String,
    revision: usize,
    model_id: Signal<String>,
    protocol: Signal<String>,
    session: Signal<String>,
    refresh: Signal<usize>,
    busy: Signal<bool>,
    generating: Signal<bool>,
    selection_error: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let state = app_context::<AppState>(cx);
    let model_csrf = state.csrf.clone();
    let mut models = crate::app::group_store(cx, "chat").list_models().await?;
    models.sort_by(|a, b| a.alias.cmp(&b.alias));
    let mut routes = crate::app::group_store(cx, "chat").list_routes().await?;
    routes.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.protocol.as_str().cmp(b.protocol.as_str()))
    });
    let mut options = Vec::new();
    for (id, label, protocols) in models
        .into_iter()
        .map(|model| (model.id.to_string(), model.alias, model.protocols))
        .chain(routes.into_iter().map(|route| {
            (
                format!("route:{}", route.id),
                format!("{} · 路由 ({})", route.name, protocol_label(route.protocol)),
                vec![route.protocol],
            )
        }))
    {
        let preferred = if id == selected {
            current.clone()
        } else {
            crate::app::chat_service::target::choose_protocol(
                &crate::app::group_store(cx, "chat"),
                &id,
                &current,
            )
            .await
            .unwrap_or_default()
        };
        let kind = Protocol::parse(&preferred).or_else(|| protocols.first().copied());
        if let Some(kind) = kind {
            let health = crate::app::chat_service::target::health(
                &crate::app::group_store(cx, "chat"),
                &id,
                kind,
            )
            .await
            .map_err(io::Error::other)?;
            options.push((id, label, health.blocked));
        }
    }
    let selected_blocked = options
        .iter()
        .any(|(id, _, blocked)| id == &selected && *blocked);
    Ok(view! {
        select(
            attrs: attributes! {
                cx =>
                id="chat-model"
                aria-label="选择聊天模型"
                class=(if selected_blocked {
                    "min-w-0 max-w-[320px] text-muted!"
                } else {
                    "min-w-0 max-w-[320px]"
                })
                :value=$(model_id.get())
                :disabled=$(if busy.get() { true } else { generating.get() })
                @change=$(async |event: Event| {
                    busy.set(true);
                    let next_model = event.target.value;
                    let next_protocol = default_protocol(
                        model_csrf.clone(),
                        next_model.clone(),
                        protocol.get(),
                    ).await;
                    if !next_protocol.is_empty() {
                        let error = switch_chat(
                            model_csrf.clone(),
                            session.get(),
                            next_model.clone(),
                            next_protocol.clone(),
                        ).await;
                        if error.is_empty() {
                            model_id.set(next_model);
                            protocol.set(next_protocol);
                            refresh.increment();
                        } else {
                            // 原生 select 已先改变 DOM，拒绝后重新应用当前选择。
                            model_id.set(model_id.get().clone());
                        }
                        selection_error.set(error);
                    } else {
                        model_id.set(model_id.get().clone());
                        selection_error.set(
                            "所选模型已不可用，当前对话和选择已保留".to_owned(

                            ),
                        );
                    }
                    busy.set(false);
                })
            },
            if !options.iter().any(|option| option.0 == selected) {
                <option value=(selected.as_str()) disabled="">
                    "原模型配置已失效，请切换模型"
                </option>
            }
            for (id, label, blocked) in &options {
                <option
                    value=(id.as_str())
                    :disabled=(*blocked)
                    class=(if *blocked { "text-muted!" } else { "" })
                >
                    (label.as_str())
                </option>
            }
        )
    })
}

#[shard("/ui/_topcoat/runtime/shards/chat-health-notice")]
pub async fn chat_health_notice(
    cx: &Cx,
    selected: String,
    current: String,
    revision: usize,
    refresh: Signal<usize>,
    busy: Signal<bool>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let state = app_context::<AppState>(cx);
    let csrf = state.csrf.clone();
    let message = signal(cx, String::new);
    let protocol = selected_protocol(&current).map_err(io::Error::other)?;
    let health = crate::app::chat_service::target::health(
        &crate::app::group_store(cx, "chat"),
        &selected,
        protocol,
    )
    .await
    .unwrap_or(crate::app::chat_service::target::Health {
        blocked: true,
        warning: true,
        severe: true,
        label: if selected.is_empty() {
            "请选择可用模型".into()
        } else {
            "模型配置已失效".into()
        },
    });
    let hint = if health.blocked {
        "当前选择和对话已保留，请重新探测或切换模型。"
    } else if health.warning {
        "最近探测未确认可用，仍可尝试发送。"
    } else {
        ""
    };
    Ok(view! {
        if health.warning {
            <div
                class=(if health.severe {
                    "mb-3 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700"
                } else if health.warning {
                    "mb-3 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800"
                } else {
                    "mb-2 px-1 text-xs text-secondary"
                })
                role="status"
                aria-live="polite"
            >
                <div
                    class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2"
                >
                    <span class="min-w-0 break-words">
                        (format!("{} · {}", protocol_label(protocol), health.label))
                    </span>
                    <div class="flex shrink-0 items-center gap-3">
                        <button
                            class="border-0 bg-transparent p-0 text-xs text-primary hover:underline"
                            type="button"
                            :disabled=$(busy.get())
                            @click=$(|_event: Event| refresh.increment())
                        >
                            "刷新状态"
                        </button>
                        if health.warning {
                            <button
                                class="border-0 bg-transparent p-0 text-xs text-primary hover:underline"
                                type="button"
                                :disabled=$(busy.get())
                                @click=$(async |_event: Event| {
                                    busy.set(true);
                                    let outcome = reprobe_chat_model(csrf, selected, current).await;
                                    busy.set(false);
                                    if outcome.is_err() {
                                        message.set(outcome.unwrap_err());
                                    } else {
                                        message.set("".to_owned());
                                        refresh.increment();
                                    }
                                })
                            >
                                "重新探测"
                            </button>
                            <label
                                for="chat-model"
                                class="cursor-pointer text-primary hover:underline"
                            >
                                "切换模型"
                            </label>
                        }
                    </div>
                </div>
                if !hint.is_empty() {
                    <p class="mt-1 mb-0 leading-5">(hint)</p>
                }
                <p class="mt-1 mb-0 leading-5" :hidden=$(message.get().is_empty())>
                    $(message.get())
                </p>
            </div>
        }
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/reprobe-chat-model")]
pub async fn reprobe_chat_model(
    cx: &Cx,
    csrf: String,
    selected: String,
    protocol: String,
) -> Result<std::result::Result<String, String>> {
    crate::app::check_csrf(cx, &csrf)?;
    let state = app_context::<AppState>(cx);
    let result = async {
        let protocol = selected_protocol(&protocol)?;
        crate::app::chat_service::target::reprobe(
            &crate::app::group_store(cx, "chat"),
            &state.health,
            &selected,
            protocol,
        )
        .await?;
        Ok("探测已完成".to_owned())
    }
    .await;
    Ok(result)
}

/// Health notifications reuse the document connection and invalidate existing pickers.
#[shard("/ui/_topcoat/runtime/shards/chat-health-updates")]
pub async fn chat_health_updates(cx: &Cx, refresh: Signal<usize>) -> Result<impl View> {
    crate::app::request_connection(cx);
    let state = app_context::<AppState>(cx);
    Ok(live! {
        let mut changed = state.health.subscribe();
        let mut update = 0usize;
        loop {
            let token = emit! {
                <span
                    hidden=""
                    :data-health-update=$(raw!(
                        "(() => { if (${update}.dehydrate() > 0) queueMicrotask(() => ${refresh}.increment()); return ${update}.dehydrate(); })()",
                        update,
                    ))
                ></span>
            }?;
            if topcoat::router::request::original_method(cx)
                != topcoat::router::Method::POST {
                break Ok(token);
            }
            if changed.changed().await.is_err() {
                break Ok(token);
            }
            update += 1;
        }
    })
}
