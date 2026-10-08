use super::*;

#[topcoat::runtime::record]
#[derive(Clone)]
pub struct ExistingModel {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub alias: String,
}

pub(super) struct Editor {
    thinking_support: Signal<String>,
    thinking_mode: Signal<String>,
    thinking_effort: Signal<String>,
    thinking_budget: Signal<String>,

    open: Signal<bool>,
    busy: Signal<bool>,
    error: Signal<String>,
    id: Signal<String>,
    version: Signal<String>,
    alias: Signal<String>,
    alias_edited: Signal<bool>,
    provider_id: Signal<String>,
    provider_name: Signal<String>,
    provider_query: Signal<String>,
    provider_menu_open: Signal<bool>,
    model_id: Signal<String>,
    input_price_per_million: Signal<String>,
    output_price_per_million: Signal<String>,
    price_summary: Signal<String>,
    model_search: Signal<String>,
    model_menu_open: Signal<bool>,
    manual_model_id: Signal<String>,
    selected_models: Signal<String>,
    draft_aliases: Signal<Vec<(String, String)>>,
    bulk_alias_warning: Signal<String>,
    candidates: Signal<Vec<ModelCandidate>>,
    candidate_status: Signal<String>,
    chat: Signal<bool>,
    responses: Signal<bool>,
    messages: Signal<bool>,
    gemini: Signal<bool>,
    supports_chat: Signal<bool>,
    supports_responses: Signal<bool>,
    supports_messages: Signal<bool>,
    supports_gemini: Signal<bool>,
    supported_protocols: Signal<String>,
    probe_protocol: Signal<String>,
    probe_tokens: Signal<String>,
    probe_busy: Signal<bool>,
    probe_success: Signal<String>,
    probe_failure: Signal<String>,
    probe_chat: Signal<bool>,
    probe_responses: Signal<bool>,
    probe_messages: Signal<bool>,
    probe_gemini: Signal<bool>,
}

impl Editor {
    pub(super) fn new(cx: &Cx) -> Self {
        Self {
            thinking_support: signal(cx, || "unknown".to_owned()),
            thinking_mode: signal(cx, String::new),
            thinking_effort: signal(cx, String::new),
            thinking_budget: signal(cx, String::new),
            open: signal(cx, || false),
            busy: signal(cx, || false),
            error: signal(cx, String::new),
            id: signal(cx, String::new),
            version: signal(cx, String::new),
            alias: signal(cx, String::new),
            alias_edited: signal(cx, || false),
            provider_id: signal(cx, String::new),
            provider_name: signal(cx, String::new),
            provider_query: signal(cx, String::new),
            provider_menu_open: signal(cx, || false),
            model_id: signal(cx, String::new),
            input_price_per_million: signal(cx, String::new),
            output_price_per_million: signal(cx, String::new),
            price_summary: signal(cx, String::new),
            model_search: signal(cx, String::new),
            model_menu_open: signal(cx, || false),
            manual_model_id: signal(cx, String::new),
            selected_models: signal(cx, || "[]".to_owned()),
            draft_aliases: signal(cx, Vec::<(String, String)>::new),
            bulk_alias_warning: signal(cx, String::new),
            candidates: signal(cx, Vec::<ModelCandidate>::new),
            candidate_status: signal(cx, String::new),
            chat: signal(cx, || false),
            responses: signal(cx, || false),
            messages: signal(cx, || false),
            gemini: signal(cx, || false),
            supports_chat: signal(cx, || false),
            supports_responses: signal(cx, || false),
            supports_messages: signal(cx, || false),
            supports_gemini: signal(cx, || false),
            supported_protocols: signal(cx, String::new),
            probe_protocol: signal(cx, String::new),
            probe_tokens: signal(cx, || "1".to_owned()),
            probe_busy: signal(cx, || false),
            probe_success: signal(cx, String::new),
            probe_failure: signal(cx, String::new),
            probe_chat: signal(cx, || false),
            probe_responses: signal(cx, || false),
            probe_messages: signal(cx, || false),
            probe_gemini: signal(cx, || false),
        }
    }
}

pub(super) fn editor_trigger(
    cx: &Cx,
    editor: &Editor,
    model: Option<&ModelMappingView>,
    provider: Option<&ProviderView>,
    plan: Option<&PricePlanView>,
) -> Attributes {
    let (id, version, alias, provider_id, model_id, chat, responses, messages, gemini) =
        if let Some(model) = model {
            (
                model.id.to_string(),
                model.version.to_string(),
                model.alias.clone(),
                model.provider_id.to_string(),
                model.upstream_model_id.clone(),
                model.protocols.contains(&Protocol::OpenAiChat),
                model.protocols.contains(&Protocol::OpenAiResponses),
                model.protocols.contains(&Protocol::AnthropicMessages),
                model.protocols.contains(&Protocol::Gemini),
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                false,
                false,
                false,
                false,
            )
        };
    let Editor {
        thinking_support,
        thinking_mode,
        thinking_effort,
        thinking_budget,

        open,
        error,
        id: selected_id,
        version: selected_version,
        alias: selected_alias,
        alias_edited,
        provider_id: selected_provider,
        provider_name: selected_provider_name,
        provider_query,
        provider_menu_open,
        model_id: selected_model,
        input_price_per_million,
        output_price_per_million,
        price_summary,
        model_search,
        model_menu_open,
        manual_model_id,
        selected_models,
        draft_aliases,
        bulk_alias_warning,
        candidates,
        candidate_status,
        chat: selected_chat,
        responses: selected_responses,
        messages: selected_messages,
        gemini: selected_gemini,
        supports_chat,
        supports_responses,
        supports_messages,
        supports_gemini,
        supported_protocols,
        probe_protocol,
        probe_tokens,
        probe_busy,
        probe_success,
        probe_failure,
        probe_chat,
        probe_responses,
        probe_messages,
        probe_gemini,
        ..
    } = editor;
    let supported_chat = provider.is_some_and(|provider| provider.paths.openai_chat.is_some());
    let supported_responses =
        provider.is_some_and(|provider| provider.paths.openai_responses.is_some());
    let supported_messages =
        provider.is_some_and(|provider| provider.paths.anthropic_messages.is_some());
    let supported_gemini = provider.is_some_and(|provider| provider.paths.gemini.is_some());
    let protocol_names = [
        (supported_chat, Protocol::OpenAiChat.as_str()),
        (supported_responses, Protocol::OpenAiResponses.as_str()),
        (supported_messages, Protocol::AnthropicMessages.as_str()),
        (supported_gemini, Protocol::Gemini.as_str()),
    ]
    .into_iter()
    .filter_map(|(enabled, name)| enabled.then_some(name))
    .collect::<Vec<_>>()
    .join("|");
    let provider_name = provider
        .map_or("", |provider| provider.name.as_str())
        .to_owned();
    let initial_probe_protocol = if chat {
        Protocol::OpenAiChat.as_str()
    } else if responses {
        Protocol::OpenAiResponses.as_str()
    } else if messages {
        Protocol::AnthropicMessages.as_str()
    } else if gemini {
        Protocol::Gemini.as_str()
    } else {
        ""
    };
    let initial_input_price = model
        .and_then(|model| model.reference_price.as_ref())
        .map_or("", |price| price.input_per_million.as_str())
        .to_owned();
    let initial_output_price = model
        .and_then(|model| model.reference_price.as_ref())
        .map_or("", |price| price.output_per_million.as_str())
        .to_owned();
    let initial_price_summary = price_label(plan, false);
    let initial_thinking = model
        .map(|model| model.thinking.clone())
        .unwrap_or_default();
    let initial_support = initial_thinking.support.as_str().to_owned();
    let initial_mode = initial_thinking.enabled.mode.unwrap_or_default();
    let initial_effort = initial_thinking.enabled.effort.unwrap_or_default();
    let initial_budget = initial_thinking
        .enabled
        .budget
        .map(|n| n.to_string())
        .unwrap_or_default();
    let empty_aliases = Vec::<(String, String)>::new();
    let empty_candidates = Vec::<ModelCandidate>::new();
    attributes! {
        cx =>
        aria-haspopup="dialog"
        aria-controls="model-dialog"
        @click=$(|_event: Event| {
            thinking_support.set(initial_support.to_owned());
            thinking_mode.set(initial_mode.to_owned());
            thinking_effort.set(initial_effort.to_owned());
            thinking_budget.set(initial_budget.to_owned());
            selected_id.set(id.to_owned());
            selected_version.set(version.to_owned());
            selected_alias.set(alias.to_owned());
            alias_edited.set(!id.is_empty());
            selected_provider.set(provider_id.to_owned());
            selected_provider_name.set(provider_name.to_owned());
            provider_query.set(provider_name.to_owned());
            provider_menu_open.set(false);
            selected_model.set(model_id.to_owned());
            input_price_per_million.set(initial_input_price.to_owned());
            output_price_per_million.set(initial_output_price.to_owned());
            price_summary.set(initial_price_summary.to_owned());
            model_search.set("".to_owned());
            model_menu_open.set(false);
            manual_model_id.set("".to_owned());
            selected_models.set("[]".to_owned());
            draft_aliases.set(empty_aliases.clone());
            bulk_alias_warning.set("".to_owned());
            candidates.set(empty_candidates.clone());
            candidate_status.set("".to_owned());
            selected_chat.set(chat);
            selected_responses.set(responses);
            selected_messages.set(messages);
            selected_gemini.set(gemini);
            supports_chat.set(supported_chat);
            supports_responses.set(supported_responses);
            supports_messages.set(supported_messages);
            supports_gemini.set(supported_gemini);
            supported_protocols.set(protocol_names.to_owned());
            probe_protocol.set(initial_probe_protocol.to_owned());
            probe_tokens.set("1".to_owned());
            probe_busy.set(false);
            probe_success.set("".to_owned());
            probe_failure.set("".to_owned());
            probe_chat.set(chat);
            probe_responses.set(responses);
            probe_messages.set(messages);
            probe_gemini.set(gemini);
            error.set("".to_owned());
            open.set(true);
        })
    }
}

#[topcoat::view::component]
async fn provider_search(
    cx: &Cx,
    providers: &[ProviderView],
    editor: &Editor,
) -> Result<impl View> {
    let _ = cx;
    let Editor {
        thinking_support,
        thinking_mode,
        thinking_effort,
        thinking_budget,

        id: editing_id,
        provider_id,
        provider_name,
        provider_query,
        provider_menu_open,
        model_id,
        input_price_per_million,
        output_price_per_million,
        alias,
        alias_edited,
        chat,
        responses,
        messages,
        gemini,
        supports_chat,
        supports_responses,
        supports_messages,
        supports_gemini,
        supported_protocols,
        selected_models,
        draft_aliases,
        bulk_alias_warning,
        candidates,
        candidate_status,
        model_search,
        model_menu_open,
        manual_model_id,
        ..
    } = editor;
    let _root_id = "model-provider-select".to_owned();
    let _list_id = "model-provider-options".to_owned();
    let unavailable: std::result::Result<Vec<ModelCandidate>, String> =
        Err("模型探测请求失败，请重试".into());
    let empty_aliases = Vec::<(String, String)>::new();
    let empty_candidates = Vec::<ModelCandidate>::new();
    Ok(view! {
        <div
            id="model-provider-select"
            class="relative min-w-0"
            @focusout=$(|_event: Event| {
                let _root_id = _root_id.to_owned();
                raw!(
                    "setTimeout(() => { const root = document.getElementById(${_root_id}.dehydrate()); if (root && !root.contains(document.activeElement)) ${provider_menu_open}.set(cx.hydrate(false)); }, 0)",
                    (),
                );
            })
        >
            <input
                id="model-provider-input"
                class="w-full"
                type="search"
                role="combobox"
                aria-label="搜索并选择 Provider"
                aria-autocomplete="list"
                aria-controls="model-provider-options"
                :aria-expanded=$(provider_menu_open.get())
                :value=$(provider_query.get())
                placeholder="搜索已启用 Provider"
                autocomplete="off"
                required=""
                @focus=$(|_event: Event| provider_menu_open.set(true))
                @input=$(|event: Event| {
                    provider_query.set(event.target.value);
                    provider_menu_open.set(true);
                })
                @keydown=$(|_event: Event| {
                    let _list_id = _list_id.to_owned();
                    raw!(
                        "{ const key = ${_event}.key.dehydrate(); if (key === 'ArrowDown' || key === 'Enter') { if (key === 'Enter') ${_event}.prevent_default(); const query = ${_event}.target.value.dehydrate().toLowerCase(); const first = [...(document.getElementById(${_list_id}.dehydrate())?.querySelectorAll('button[role=option]') ?? [])].find(option => option.textContent.toLowerCase().includes(query)); if (first) { if (key === 'ArrowDown') ${_event}.prevent_default(); if (key === 'Enter') first.click(); else first.focus(); } } else if (key === 'Escape') { ${provider_menu_open}.set(cx.hydrate(false)); } }",
                        (),
                    );
                })
            >
            <div
                id="model-provider-options"
                class="absolute z-30 mt-1 max-h-48 w-full overflow-y-auto rounded-md border border-control-border bg-white p-1 shadow-[0_6px_16px_rgba(0,0,0,0.08)]"
                role="listbox"
                aria-label="已启用 Provider"
                :hidden=$(!provider_menu_open.get())
            >
                #[key(provider.id)]
                for provider in providers {
                    let option_id = provider.id.to_string();
                    let option_name = provider.name.clone();
                    let has_chat = provider.paths.openai_chat.is_some();
                    let has_responses = provider.paths.openai_responses.is_some();
                    let has_messages = provider.paths.anthropic_messages.is_some();
                    let has_gemini = provider.paths.gemini.is_some();
                    let protocol_names = provider
                        .paths
                        .supported()
                        .into_iter()
                        .map(Protocol::as_str)
                        .collect::<Vec<_>>()
                        .join("|");
                    let only_one = provider.paths.supported().len() == 1;
                    let default_chat = only_one && has_chat;
                    let default_responses = only_one && has_responses;
                    let default_messages = only_one && has_messages;
                    let default_gemini = only_one && has_gemini;
                    let unavailable = unavailable.clone();
                    let confirm_id = format!("switch-model-provider-{}", provider.id);
                    let confirm_title = format!(
                        "切换到「{}」？未保存的模型会清空。",
                        provider.name,
                    );
                    let option_class = "block w-full rounded-md border-0! bg-transparent! px-3 py-2 text-left text-sm leading-5 text-heading shadow-none! hover:bg-primary-soft! focus:bg-primary-soft! focus:outline-none aria-selected:text-primary data-[filtered]:hidden";
                    let choose = attributes! {
                        cx =>
                        @click=$(async |_event: Event| {
                            thinking_support.set("unknown".to_owned());
                            thinking_mode.set("".to_owned());
                            thinking_effort.set("".to_owned());
                            thinking_budget.set("".to_owned());
                            provider_id.set(option_id.to_owned());
                            provider_name.set(option_name.to_owned());
                            provider_query.set(option_name.to_owned());
                            provider_menu_open.set(false);
                            model_id.set("".to_owned());
                            input_price_per_million.set("".to_owned());
                            output_price_per_million.set("".to_owned());
                            if !alias_edited.get() {
                                alias.set("".to_owned());
                            }
                            chat.set(default_chat);
                            responses.set(default_responses);
                            messages.set(default_messages);
                            gemini.set(default_gemini);
                            supports_chat.set(has_chat);
                            supports_responses.set(has_responses);
                            supports_messages.set(has_messages);
                            supports_gemini.set(has_gemini);
                            supported_protocols.set(protocol_names.to_owned());
                            selected_models.set("[]".to_owned());
                            draft_aliases.set(empty_aliases.clone());
                            bulk_alias_warning.set("".to_owned());
                            candidates.set(empty_candidates.clone());
                            candidate_status.set("".to_owned());
                            model_search.set("".to_owned());
                            model_menu_open.set(false);
                            manual_model_id.set("".to_owned());
                            let creating = editing_id.get().is_empty();
                            if creating {
                                candidate_status.set("loading".to_owned());
                            }
                            let result = raw!(
                                "await Promise.resolve(${creating}.dehydrate() ? ${load_model_candidates}.call(${option_id}) : ${unavailable}).catch(() => ${unavailable})",
                                unavailable.clone(),
                            );
                            if creating {
                                if provider_id.get() == option_id {
                                    if result.is_ok() {
                                        candidates.set(result.unwrap());
                                        candidate_status.set("".to_owned());
                                    } else {
                                        candidate_status.set(result.unwrap_err());
                                    }
                                }
                            }
                        })
                    };
                    <button
                        class=(option_class)
                        type="button"
                        role="option"
                        :aria-selected=$(provider_id.get() == option_id)
                        :data-filtered=$(if provider_query.get().is_empty() {
                            false
                        } else {
                            raw!(
                                "!${option_name}.dehydrate().toLowerCase().includes(${provider_query}.get().dehydrate().toLowerCase())",
                                false,
                            )
                        })
                        :hidden=$(if selected_models.get() == "[]" {
                            false
                        } else {
                            provider_id.get() != option_id
                        })
                        (choose.clone())
                    >
                        (provider.name.as_str())
                    </button>
                    <button
                        class=(option_class)
                        type="button"
                        role="option"
                        :aria-selected=$(provider_id.get() == option_id)
                        :data-filtered=$(if provider_query.get().is_empty() {
                            false
                        } else {
                            raw!(
                                "!${option_name}.dehydrate().toLowerCase().includes(${provider_query}.get().dehydrate().toLowerCase())",
                                false,
                            )
                        })
                        :hidden=$(if selected_models.get() == "[]" {
                            true
                        } else {
                            provider_id.get() == option_id
                        })
                        (popconfirm_trigger_attributes(cx, &confirm_id))
                    >
                        (provider.name.as_str())
                    </button>
                    popconfirm(
                        id: confirm_id.as_str(),
                        title: confirm_title.as_str(),
                        language: UiLanguage::ChineseSimplified,
                        attrs: attributes! {
                            cx =>
                            @click=$(|_event: Event| provider_query.set(
                                    provider_name.get(),
                                ))
                        },
                        <button
                            class="gr-button gr-button-danger"
                            type="button"
                            (choose)
                        >
                            "确认切换"
                        </button>
                    )
                }
            </div>
        </div>
    })
}

fn price_summary(input: Option<&str>, output: Option<&str>) -> String {
    match input.zip(output) {
        Some((input, output)) => format!("${input}/${output}"),
        None => "-".to_owned(),
    }
}

#[topcoat::view::component]
async fn draft_model_row(
    cx: &Cx,
    model_id: &str,
    index: usize,
    provider_name: &str,
    selected: Signal<String>,
    draft_aliases: Signal<Vec<(String, String)>>,
    supports_chat: bool,
    supports_responses: bool,
    supports_messages: bool,
    supports_gemini: bool,
    default_chat: bool,
    default_responses: bool,
    default_messages: bool,
    default_gemini: bool,
    price: Option<&ModelCandidate>,
) -> Result<impl View> {
    let initial_alias = draft_aliases
        .get_untracked()
        .into_iter()
        .find(|(id, _)| id == model_id)
        .map(|(_, alias)| alias)
        .unwrap_or_else(|| format!("{provider_name}/{model_id}"));
    let alias = signal(cx, || initial_alias);
    let chat = signal(cx, || default_chat);
    let responses = signal(cx, || default_responses);
    let messages = signal(cx, || default_messages);
    let gemini = signal(cx, || default_gemini);
    let row_id = model_id.to_owned();
    let input_price = price.and_then(|price| price.input_price_per_million.as_deref());
    let output_price = price.and_then(|price| price.output_price_per_million.as_deref());
    let price_label = price_summary(input_price, output_price);
    let menu_id = format!("model-protocols-{index}");
    let menu_anchor = format!("position-anchor: --gr-{menu_id}");
    Ok(view! {
        <div
            class="grid grid-cols-[minmax(0,1.1fr)_minmax(0,1.2fr)_minmax(126px,.7fr)_minmax(146px,.8fr)_32px] items-center gap-2.5 py-2 max-[760px]:grid-cols-[minmax(0,1fr)_32px]"
        >
            <input type="hidden" name="model_id" value=(model_id)>
            if let Some(value) = input_price {
                <input
                    type="hidden"
                    name=(format!("input-price-{index}"))
                    value=(value)
                >
            }
            if let Some(value) = output_price {
                <input
                    type="hidden"
                    name=(format!("output-price-{index}"))
                    value=(value)
                >
            }
            <div
                class="min-w-0 truncate text-sm text-heading max-[760px]:col-span-1"
                title=(model_id)
            >
                <span class="hidden text-xs text-secondary max-[760px]:block">
                    "模型 ID"
                </span>
                (model_id)
            </div>
            <div class="min-w-0 max-[760px]:col-span-2 max-[760px]:row-start-2">
                <label
                    class="hidden text-xs text-secondary max-[760px]:mb-1 max-[760px]:block"
                    for=(format!("alias-{index}"))
                >
                    "模型标识"
                </label>
                <input
                    id=(format!("alias-{index}"))
                    name=(format!("alias-{index}"))
                    class="h-8 w-full text-[13px]"
                    type="text"
                    :value=$(alias.get())
                    maxlength="200"
                    aria-label=(format!("{model_id} 的模型标识"))
                    @input=$(|event: Event| {
                        alias.set(event.target.value);
                        draft_aliases.set(
                            raw!(
                                "(() => { const entries = new Map(${draft_aliases}.get().dehydrate().v); entries.set(${row_id}.dehydrate(), ${event}.target.value.dehydrate()); return cx.hydrate({ ...${draft_aliases}.get().dehydrate(), v: [...entries] }); })()",
                                Vec::<(String, String)>::new(),
                            ),
                        );
                    })
                >
            </div>
            <div class="min-w-0 max-[760px]:row-start-3">
                <span
                    class="hidden text-xs text-secondary max-[760px]:mb-1 max-[760px]:block"
                >
                    "协议"
                </span>
                <button
                    class="flex h-8 w-full items-center justify-between gap-2 rounded-md border border-control-border bg-white px-2.5 text-left text-[13px] text-heading hover:border-primary-hover focus-visible:outline-2 focus-visible:outline-primary"
                    type="button"
                    aria-label=(format!("{model_id} 的协议"))
                    (anchored_menu_trigger_attributes(cx, &menu_id))
                >
                    <span class="truncate">
                        $(if chat.get() {
                            if responses.get() {
                                if messages.get() {
                                    if gemini.get() { "Chat +3" } else { "Chat +2" }
                                } else if gemini.get() {
                                    "Chat +2"
                                } else {
                                    "Chat +1"
                                }
                            } else if messages.get() {
                                if gemini.get() { "Chat +2" } else { "Chat +1" }
                            } else if gemini.get() {
                                "Chat +1"
                            } else {
                                "Chat"
                            }
                        } else if responses.get() {
                            if messages.get() {
                                if gemini.get() { "Responses +2" } else { "Responses +1" }
                            } else if gemini.get() {
                                "Responses +1"
                            } else {
                                "Responses"
                            }
                        } else if messages.get() {
                            if gemini.get() { "Messages +1" } else { "Messages" }
                        } else if gemini.get() {
                            "Gemini"
                        } else {
                            "选择协议"
                        })
                    </span>
                    <span class="text-secondary" aria-hidden="true">"⌄"</span>
                </button>
                <div
                    id=(menu_id.as_str())
                    class="gr-anchored-menu fixed inset-auto m-0 mt-1.5 min-w-[150px] rounded-md border border-border bg-white p-1.5 text-heading shadow-lg"
                    popover="auto"
                    role="group"
                    aria-label=(format!("{model_id} 支持的协议"))
                    style=(menu_anchor)
                >
                    if supports_chat {
                        <label
                            class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-[13px] hover:bg-primary-soft"
                        >
                            <input
                                type="checkbox"
                                name=(format!("chat-{index}"))
                                :checked=$(chat.get())
                                @change=$(|event: Event| chat.set(event.target.checked))
                            >
                            "Chat"
                        </label>
                    }
                    if supports_responses {
                        <label
                            class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-[13px] hover:bg-primary-soft"
                        >
                            <input
                                type="checkbox"
                                name=(format!("responses-{index}"))
                                :checked=$(responses.get())
                                @change=$(|event: Event| responses.set(event.target.checked))
                            >
                            "Responses"
                        </label>
                    }
                    if supports_messages {
                        <label
                            class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-[13px] hover:bg-primary-soft"
                        >
                            <input
                                type="checkbox"
                                name=(format!("messages-{index}"))
                                :checked=$(messages.get())
                                @change=$(|event: Event| messages.set(event.target.checked))
                            >
                            "Messages"
                        </label>
                    }
                    if supports_gemini {
                        <label
                            class="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-[13px] hover:bg-primary-soft"
                        >
                            <input
                                type="checkbox"
                                name=(format!("gemini-{index}"))
                                :checked=$(gemini.get())
                                @change=$(|event: Event| gemini.set(event.target.checked))
                            >
                            "Gemini"
                        </label>
                    }
                </div>
            </div>
            <div
                class="min-w-0 truncate text-[13px] tabular-nums text-heading max-[760px]:row-start-3"
                title=(price_label.as_str())
            >
                <span
                    class="hidden text-xs text-secondary max-[760px]:mb-1 max-[760px]:block"
                >
                    "In/Out(M)"
                </span>
                (price_label.as_str())
            </div>
            <button
                class="inline-flex size-8 shrink-0 items-center justify-center self-center rounded-full border border-control-border bg-white p-0 text-muted shadow-sm transition-colors duration-150 hover:border-[#ffccc7] hover:bg-[#fff2f0] hover:text-[#cf1322] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#91caff] max-[760px]:col-start-2 max-[760px]:row-start-1"
                type="button"
                aria-label=(format!("移除 {model_id}"))
                title="移除模型"
                @click=$(async |_event: Event| {
                    selected.set(
                        remove_draft_model(selected.get(), row_id.to_owned()).await,
                    );
                    draft_aliases.set(
                        raw!(
                            "(() => { const entries = new Map(${draft_aliases}.get().dehydrate().v); entries.delete(${row_id}.dehydrate()); return cx.hydrate({ ...${draft_aliases}.get().dehydrate(), v: [...entries] }); })()",
                            Vec::<(String, String)>::new(),
                        ),
                    );
                })
            >
                icon(data: CLOSE_OUTLINED, size: 14)
            </button>
        </div>
    })
}

#[shard("/ui/_topcoat/runtime/shards/model-draft")]
pub async fn model_draft(
    cx: &Cx,
    provider_id: Signal<String>,
    provider_name: Signal<String>,
    candidates: Signal<Vec<ModelCandidate>>,
    candidate_status: Signal<String>,
    selected: Signal<String>,
    draft_aliases: Signal<Vec<(String, String)>>,
    bulk_alias_warning: Signal<String>,
    existing_models: Vec<ExistingModel>,
    search: Signal<String>,
    menu_open: Signal<bool>,
    manual_model_id: Signal<String>,
    supported_protocols: Signal<String>,
) -> Result<impl View> {
    let manual_open = signal(cx, || false);
    let protocols = supported_protocols.get();
    let supports_chat = protocols
        .split('|')
        .any(|name| name == Protocol::OpenAiChat.as_str());
    let supports_responses = protocols
        .split('|')
        .any(|name| name == Protocol::OpenAiResponses.as_str());
    let supports_messages = protocols
        .split('|')
        .any(|name| name == Protocol::AnthropicMessages.as_str());
    let supports_gemini = protocols
        .split('|')
        .any(|name| name == Protocol::Gemini.as_str());
    let available = candidates.get();
    let discovered_count = available.len();
    let selected_ids: Vec<String> = serde_json::from_str(&selected.get()).unwrap_or_default();
    let mut chosen = HashSet::new();
    let rows: Vec<_> = selected_ids
        .into_iter()
        .filter(|id| chosen.insert(id.clone()))
        .collect();
    let aliases = draft_aliases.get();
    let duplicate_model_ids: Vec<_> = rows
        .iter()
        .filter(|model_id| {
            existing_models
                .iter()
                .any(|saved| saved.provider_id == provider_id.get() && saved.model_id == **model_id)
        })
        .map(|model_id| format!("「{model_id}」"))
        .collect();
    let initial_model_note = if duplicate_model_ids.is_empty() {
        String::new()
    } else {
        format!(
            "模型 ID {} 已在当前 Provider 中导入；使用其他模型标识仍可继续。",
            duplicate_model_ids.join("、")
        )
    };
    let mut initial_alias_warning = String::new();
    let mut seen_aliases = HashSet::new();
    for model_id in &rows {
        let alias = aliases
            .iter()
            .find(|(id, _)| id == model_id)
            .map(|(_, alias)| alias.trim())
            .filter(|alias| !alias.is_empty())
            .map_or_else(
                || format!("{}/{}", provider_name.get(), model_id),
                str::to_owned,
            );
        if existing_models.iter().any(|saved| saved.alias == alias) {
            initial_alias_warning = format!("模型标识「{alias}」已存在，请修改后再导入");
            break;
        }
        if !seen_aliases.insert(alias.clone()) {
            initial_alias_warning = format!("本次导入的模型标识「{alias}」重复");
            break;
        }
    }
    let mut seen = HashSet::new();
    let mut options: Vec<SearchOption> = available
        .iter()
        .filter(|candidate| seen.insert(candidate.id.clone()))
        .map(|candidate| SearchOption {
            value: candidate.id.clone(),
            label: candidate.id.clone(),
        })
        .collect();
    for id in &rows {
        if seen.insert(id.clone()) {
            options.push(SearchOption {
                value: id.clone(),
                label: id.clone(),
            });
        }
    }
    let only_one = [
        supports_chat,
        supports_responses,
        supports_messages,
        supports_gemini,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count()
        == 1;
    let provider_key = provider_id.get();
    let provider_label = provider_name.get();
    let has_chat = supports_chat;
    let has_responses = supports_responses;
    let has_messages = supports_messages;
    let has_gemini = supports_gemini;
    Ok(view! {
        <div class="min-w-0">
            <label
                class="mb-2 block text-sm font-semibold text-heading"
                for="model-select-input"
            >
                "选择上游模型"
                <span class="ml-1 text-[#ff4d4f]">"*"</span>
            </label>
            if options.is_empty() {
                <input
                    id="model-select-input"
                    class="w-full"
                    type="search"
                    placeholder="搜索上游模型"
                    disabled=""
                >
            } else {
                search_multi_select(
                    id: "model-select",
                    options: &options,
                    selected: &selected,
                    search: &search,
                    open: &menu_open,
                    label: Some("上游模型"),
                    placeholder: Some("搜索上游模型"),
                    language: UiLanguage::ChineseSimplified
                )
            }
            <div class="mt-2 flex flex-wrap items-center justify-between gap-2">
                <p class="m-0 text-xs text-secondary" role="status">
                    if provider_id.get().is_empty() {
                        "先选择 Provider，再选择或手动添加模型。"
                    } else if candidate_status.get() == "loading" {
                        "正在探测模型列表，也可以先手动添加。"
                    } else if discovered_count == 0 {
                        "模型列表暂无可选项，也可以手动添加。"
                    } else {
                        (format!(
                            "探测到 {discovered_count} 个模型，可搜索并一次选择多个。",
                        ))
                    }
                </p>
                <button
                    class="shrink-0 border-0 bg-transparent p-0 text-xs font-medium text-primary hover:underline"
                    type="button"
                    :hidden=$(provider_id.get().is_empty())
                    @click=$(|_event: Event| manual_open.set(!manual_open.get()))
                >
                    $(if manual_open.get() {
                        "收起手动添加"
                    } else {
                        "+ 手动添加模型 ID"
                    })
                </button>
            </div>
            <p
                class="mt-2 mb-0 text-xs text-[#cf1322]"
                role="alert"
                :hidden=$(if candidate_status.get() == "loading" {
                    true
                } else {
                    candidate_status.get().is_empty()
                })
            >
                $(candidate_status.get())
            </p>
            <div
                class="mt-3 flex items-center gap-2 rounded-md border border-border bg-surface/40 p-2 max-[480px]:flex-wrap"
                :hidden=$(!manual_open.get())
            >
                <input
                    id="manual-model-id"
                    class="min-w-0 flex-1 max-[480px]:basis-full"
                    type="text"
                    aria-label="手动输入上游模型 ID"
                    placeholder="输入上游模型 ID"
                    maxlength="200"
                    autocomplete="off"
                    :value=$(manual_model_id.get())
                    @input=$(|event: Event| manual_model_id.set(event.target.value))
                    @keydown=$(|_event: Event| {
                        raw!(
                            "if (${_event}.key.dehydrate() === 'Enter') { ${_event}.prevent_default(); document.getElementById('manual-model-add')?.click(); }",
                            (),
                        );
                    })
                >
                <button
                    id="manual-model-add"
                    class=(BUTTON)
                    type="button"
                    :disabled=$(manual_model_id.get().trim().is_empty())
                    @click=$(async |_event: Event| {
                        selected.set(
                            add_draft_model(selected.get(), manual_model_id.get()).await,
                        );
                        manual_model_id.set("".to_owned());
                        manual_open.set(false);
                    })
                >
                    "添加"
                </button>
            </div>
            <p class="mt-2 mb-0 text-xs text-secondary" :hidden=$(!manual_open.get())>
                "手动添加的模型不会经过列表校验；保存后可在编辑页探测可用性。"
            </p>
        </div>
        if !rows.is_empty() {
            <div class="col-span-full mt-2">
                <h3 class="mb-3 text-sm font-semibold text-heading">
                    (format!("待导入模型（{}）", rows.len()))
                </h3>
                <div
                    class="grid grid-cols-[minmax(0,1.1fr)_minmax(0,1.2fr)_minmax(126px,.7fr)_minmax(146px,.8fr)_32px] gap-2.5 border-b border-border pb-2 text-xs font-medium text-secondary max-[760px]:hidden"
                >
                    <span>"模型 ID"</span>
                    <span>"模型标识"</span>
                    <span>"协议"</span>
                    <span title="输入/输出，每百万 token 的美元参考价">
                        "In/Out(M)"
                    </span>
                    <span class="sr-only">"操作"</span>
                </div>
                <div class="divide-y divide-border">
                    #[key((provider_key.as_str(), row_id.as_str()))]
                    for (index, row_id) in rows.iter().enumerate() {
                        draft_model_row(
                            model_id: row_id.as_str(),
                            index: index,
                            provider_name: provider_label.as_str(),
                            selected: selected.clone(),
                            draft_aliases: draft_aliases.clone(),
                            supports_chat: has_chat,
                            supports_responses: has_responses,
                            supports_messages: has_messages,
                            supports_gemini: has_gemini,
                            default_chat: only_one && has_chat,
                            default_responses: only_one && has_responses,
                            default_messages: only_one && has_messages,
                            default_gemini: only_one && has_gemini,
                            price: available
                                .iter()
                                .find(|candidate| candidate.id == *row_id)
                        )
                    }
                </div>
                <p class="mt-2 mb-0 text-xs text-[#ad6800] empty:hidden" role="status">
                    $({
                        let _selected = selected.get();
                        raw!(
                            "(() => { const saved = ${existing_models}.dehydrate().v; const selected = JSON.parse(String(${selected}.get())); const provider = String(${provider_id}.get()); const duplicates = selected.filter((modelId) => saved.some((record) => record.v.provider_id === provider && record.v.model_id === modelId)); return duplicates.length ? '模型 ID ' + duplicates.map((modelId) => '「' + modelId + '」').join('、') + ' 已在当前 Provider 中导入；使用其他模型标识仍可继续。' : ''; })()",
                            initial_model_note.clone(),
                        )
                    })
                </p>
            </div>
        }
        <p
            class="col-span-full mt-2 mb-0 text-xs text-[#cf1322] empty:hidden"
            role="alert"
        >
            $({
                let _selected = selected.get();
                let _aliases = draft_aliases.get();
                raw!(
                    "(() => { const saved = ${existing_models}.dehydrate().v; const selected = JSON.parse(String(${selected}.get())); const entries = new Map(${draft_aliases}.get().dehydrate().v); const provider = String(${provider_name}.get()); const seen = new Set(); const modelIds = new Set(); let warning = ''; for (const modelId of selected) { if (modelIds.has(modelId)) continue; modelIds.add(modelId); const current = String(entries.get(modelId) ?? '').trim() || provider + '/' + modelId; if (saved.some((record) => record.v.alias === current)) { warning = '模型标识「' + current + '」已存在，请修改后再导入'; break; } if (seen.has(current)) { warning = '本次导入的模型标识「' + current + '」重复'; break; } seen.add(current); } ${bulk_alias_warning}.set(cx.hydrate(warning)); return warning; })()",
                    initial_alias_warning.clone(),
                )
            })
        </p>
    })
}

#[topcoat::view::component]
pub(super) async fn model_editor(
    cx: &Cx,
    editor: &Editor,
    providers: &[ProviderView],
    all_models: &[ModelMappingView],
    csrf: &str,
    success: &Signal<String>,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let Editor {
        thinking_support,
        thinking_mode,
        thinking_effort,
        thinking_budget,

        open,
        busy,
        error,
        id,
        version,
        alias,
        alias_edited,
        provider_id,
        provider_name,
        provider_query,
        model_id,
        input_price_per_million,
        output_price_per_million,
        price_summary,
        chat,
        responses,
        messages,
        gemini,
        supports_chat,
        supports_responses,
        supports_messages,
        supports_gemini,
        supported_protocols,
        candidates,
        selected_models,
        draft_aliases,
        bulk_alias_warning,
        model_search,
        model_menu_open,
        manual_model_id,
        candidate_status,
        probe_protocol,
        probe_tokens,
        probe_busy,
        probe_success,
        probe_failure,
        probe_chat,
        probe_responses,
        probe_messages,
        probe_gemini,
        ..
    } = editor;
    let close = native_dialog_close_attributes(cx, "model-dialog");
    let unavailable: Outcome = Err("保存请求失败，请刷新后重试".into());
    let probe_unavailable: Outcome = Err("探测请求失败，请重试".into());
    let existing_models: Vec<_> = all_models
        .iter()
        .map(|model| ExistingModel {
            id: model.id.to_string(),
            provider_id: model.provider_id.to_string(),
            model_id: model.upstream_model_id.clone(),
            alias: model.alias.clone(),
        })
        .collect();
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new("model-dialog", "模型配置"),
            open: Some(open),
            busy: busy,
            language: UiLanguage::ChineseSimplified,
            attrs: attributes! {
                class="w-[min(960px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:px-6 [&_.gr-native-dialog-header]:py-4"
            },
            <form
                id="model-editor-form"
                class="m-0 flex min-h-0 max-h-[calc(100dvh_-_48px)] flex-col"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() {
                        return;
                    }
                    if provider_id.get().is_empty() {
                        error.set("请从下拉列表选择 Provider".to_owned());
                        return;
                    }
                    if provider_query.get() != provider_name.get() {
                        error.set("请从下拉列表选择 Provider".to_owned());
                        return;
                    }
                    busy.set(true);
                    error.set("".to_owned());
                    // Topcoat renders each draft row with signals; its runtime
                    // cannot collect a dynamic form list without FormData.
                    let result = raw!(
                        "await Promise.resolve(${id}.get().dehydrate() ? ${save_model}.call(${csrf}, cx.hydrate(JSON.stringify({id: ${id}.get().dehydrate(), version: ${version}.get().dehydrate(), alias: ${alias}.get().dehydrate(), provider_id: ${provider_id}.get().dehydrate(), upstream_model_id: ${model_id}.get().dehydrate(), chat: ${chat}.get().dehydrate(), responses: ${responses}.get().dehydrate(), messages: ${messages}.get().dehydrate(), gemini: ${gemini}.get().dehydrate(), input_price_per_million: ${input_price_per_million}.get().dehydrate(), output_price_per_million: ${output_price_per_million}.get().dehydrate(), thinking_support: ${thinking_support}.get().dehydrate(), thinking_mode: ${thinking_mode}.get().dehydrate(), thinking_effort: ${thinking_effort}.get().dehydrate(), thinking_budget: ${thinking_budget}.get().dehydrate()}))) : (() => { const form = new FormData(document.getElementById('model-editor-form')); const rows = form.getAll('model_id').map((model_id, index) => ({model_id, alias: form.get('alias-' + index) || '', chat: form.has('chat-' + index), responses: form.has('responses-' + index), messages: form.has('messages-' + index), gemini: form.has('gemini-' + index), input_price_per_million: form.get('input-price-' + index), output_price_per_million: form.get('output-price-' + index)})); return ${save_models}.call(${csrf}, ${provider_id}.get(), cx.hydrate(JSON.stringify(rows))); })()).catch(() => ${unavailable})",
                        unavailable.clone(),
                    );
                    busy.set(false);
                    if result.is_ok() {
                        open.set(false);
                        success.set(result.unwrap());
                        refresh.increment();
                    } else {
                        error.set(result.unwrap_err());
                    }
                })
            >
                <div
                    class="min-h-[min(320px,calc(100dvh_-_180px))] overflow-y-auto p-6"
                >
                    <div
                        class="mb-5 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]"
                        role="alert"
                        :hidden=$(error.get().is_empty())
                    >
                        $(error.get())
                    </div>
                    <div
                        class="grid grid-cols-2 items-start gap-x-5 gap-y-4 max-[760px]:grid-cols-1"
                    >
                        <div class="min-w-0">
                            <label
                                class="mb-2 block text-sm font-semibold"
                                for="model-provider-input"
                            >
                                "选择已启用 Provider"
                                <span class="ml-1 text-[#ff4d4f]">"*"</span>
                            </label>
                            provider_search(providers: providers, editor: editor)
                        </div>
                        <div class="contents" :hidden=$(!id.get().is_empty())>
                            model_draft(
                                provider_id: provider_id.clone(),
                                provider_name: provider_name.clone(),
                                candidates: candidates.clone(),
                                candidate_status: candidate_status.clone(),
                                selected: selected_models.clone(),
                                draft_aliases: draft_aliases.clone(),
                                bulk_alias_warning: bulk_alias_warning.clone(),
                                existing_models: existing_models.clone(),
                                search: model_search.clone(),
                                menu_open: model_menu_open.clone(),
                                manual_model_id: manual_model_id.clone(),
                                supported_protocols: supported_protocols.clone()
                            )
                        </div>
                    </div>
                    <div :hidden=$(id.get().is_empty())>
                        <div class="mt-6">
                            form_field(
                                config: FormFieldConfig::new("model-id", "上游模型 ID").required(

                                ),
                                <input
                                    id="model-id"
                                    list="model-candidates"
                                    autocomplete="off"
                                    :value=$(model_id.get())
                                    @input=$(|event: Event| {
                                        if model_id.get() != event.target.value {
                                            thinking_support.set("unknown".to_owned());
                                            thinking_mode.set("".to_owned());
                                            thinking_effort.set("".to_owned());
                                            thinking_budget.set("".to_owned());
                                            input_price_per_million.set("".to_owned());
                                            output_price_per_million.set("".to_owned());
                                        }
                                        model_id.set(event.target.value);
                                        if !alias_edited.get() {
                                            alias.set("".to_owned());
                                            if !model_id.get().is_empty() {
                                                alias.set(provider_name.get());
                                                alias.push_str("/");
                                                alias.push_str(model_id.get());
                                            }
                                        }
                                    })
                                    placeholder="搜索探测结果，或手动输入模型 ID"
                                    :disabled=$(provider_id.get().is_empty())
                                >
                            )
                        </div>
                        model_candidates(
                            provider_id: $(provider_id.get()),
                            open: $(if id.get().is_empty() { false } else { open.get() })
                        )
                        <div
                            class="mt-4 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px]"
                        >
                            <span class="font-medium text-secondary">
                                "参考价格 · In/Out(M)"
                            </span>
                            <span class="font-medium tabular-nums text-heading">
                                $(price_summary.get())
                            </span>
                            <span class="text-secondary">
                                "可在模型列表的参考价格列编辑。"
                            </span>
                        </div>
                        <div class="mt-6">
                            form_field(
                                config: FormFieldConfig::new("model-alias", "模型标识"),
                                <input
                                    id="model-alias"
                                    :value=$(alias.get())
                                    @input=$(|event: Event| {
                                        alias.set(event.target.value);
                                        alias_edited.set(true);
                                    })
                                    placeholder="Provider名称/模型ID"
                                    maxlength="200"
                                >
                            )
                        </div>
                        <p
                            class="mt-2 mb-0 text-xs text-[#cf1322] empty:hidden"
                            role="alert"
                        >
                            $({
                                let _alias = alias.get();
                                let _model_id = model_id.get();
                                raw!(
                                    "(() => { const current = String(${alias}.get()).trim() || String(${provider_name}.get()) + '/' + String(${model_id}.get()); if (!current) return ''; const ownId = String(${id}.get()); return ${existing_models}.dehydrate().v.some((record) => record.v.id !== ownId && record.v.alias === current) ? '模型标识「' + current + '」已存在，请修改后再保存' : ''; })()",
                                    String::new(),
                                )
                            })
                        </p>
                        thinking::editor(
                            support: thinking_support.clone(),
                            mode: thinking_mode.clone(),
                            effort: thinking_effort.clone(),
                            budget: thinking_budget.clone()
                        )
                        <h3 class="mt-6 mb-3 text-sm font-semibold">
                            "选择可用协议（至少一个）"
                        </h3>
                        <div class="flex flex-wrap gap-4 text-sm text-heading">
                            <label
                                class="flex items-center gap-2"
                                :hidden=$(!supports_chat.get())
                            >
                                <input
                                    type="checkbox"
                                    :checked=$(chat.get())
                                    @change=$(|event: Event| chat.set(event.target.checked))
                                >
                                "Chat"
                            </label>
                            <label
                                class="flex items-center gap-2"
                                :hidden=$(!supports_responses.get())
                            >
                                <input
                                    type="checkbox"
                                    :checked=$(responses.get())
                                    @change=$(|event: Event| responses.set(event.target.checked))
                                >
                                "Responses"
                            </label>
                            <label
                                class="flex items-center gap-2"
                                :hidden=$(!supports_messages.get())
                            >
                                <input
                                    type="checkbox"
                                    :checked=$(messages.get())
                                    @change=$(|event: Event| messages.set(event.target.checked))
                                >
                                "Messages"
                            </label>
                            <label
                                class="flex items-center gap-2"
                                :hidden=$(!supports_gemini.get())
                            >
                                <input
                                    type="checkbox"
                                    :checked=$(gemini.get())
                                    @change=$(|event: Event| gemini.set(event.target.checked))
                                >
                                "Gemini"
                            </label>
                        </div>
                        <div
                            class="mt-6 rounded-lg border border-border bg-surface/40 p-4"
                        >
                            <h3 class="m-0 text-sm font-semibold text-heading">
                                "可用性探测"
                            </h3>
                            <p class="mt-1 mb-4 text-xs text-secondary">
                                "对已保存的模型发一次极短请求。按已保存的协议和上游路径探测，不修改模型配置。"
                            </p>
                            <div
                                class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-end gap-3 max-[640px]:grid-cols-1"
                            >
                                <label class="min-w-0 text-xs font-medium text-heading">
                                    "探测协议"
                                    <select
                                        class="mt-2 w-full"
                                        :value=$(probe_protocol.get())
                                        @change=$(|event: Event| {
                                            probe_protocol.set(event.target.value);
                                            probe_success.set("".to_owned());
                                            probe_failure.set("".to_owned());
                                        })
                                    >
                                        <option
                                            value="openai_chat"
                                            :disabled=$(!probe_chat.get())
                                            :hidden=$(!probe_chat.get())
                                        >
                                            "Chat"
                                        </option>
                                        <option
                                            value="openai_responses"
                                            :disabled=$(!probe_responses.get())
                                            :hidden=$(!probe_responses.get())
                                        >
                                            "Responses"
                                        </option>
                                        <option
                                            value="anthropic_messages"
                                            :disabled=$(!probe_messages.get())
                                            :hidden=$(!probe_messages.get())
                                        >
                                            "Messages"
                                        </option>
                                        <option
                                            value="gemini"
                                            :disabled=$(!probe_gemini.get())
                                            :hidden=$(!probe_gemini.get())
                                        >
                                            "Gemini"
                                        </option>
                                    </select>
                                </label>
                                <label class="min-w-0 text-xs font-medium text-heading">
                                    "输出上限（token）"
                                    <input
                                        class="mt-2 w-full"
                                        type="number"
                                        min="1"
                                        max="1024"
                                        step="1"
                                        :value=$(probe_tokens.get())
                                        @input=$(|event: Event| {
                                            probe_tokens.set(event.target.value);
                                            probe_success.set("".to_owned());
                                            probe_failure.set("".to_owned());
                                        })
                                    >
                                </label>
                                <button
                                    class=(class!(BUTTON, "h-10"))
                                    type="button"
                                    :disabled=$(probe_busy.get())
                                    @click=$(async |_event: Event| {
                                        if busy.get() {
                                            return;
                                        }
                                        if probe_busy.get() {
                                            return;
                                        }
                                        busy.set(true);
                                        probe_busy.set(true);
                                        probe_success.set("".to_owned());
                                        probe_failure.set("".to_owned());
                                        let result = raw!(
                                            "await Promise.resolve(${probe_saved_model}.call(${csrf}, ${id}.get(), ${probe_protocol}.get(), ${probe_tokens}.get())).catch(() => ${probe_unavailable})",
                                            probe_unavailable.clone(),
                                        );
                                        probe_busy.set(false);
                                        busy.set(false);
                                        if result.is_ok() {
                                            probe_success.set(result.unwrap());
                                        } else {
                                            probe_failure.set(result.unwrap_err());
                                        }
                                    })
                                >
                                    $(if probe_busy.get() {
                                        "探测中…"
                                    } else {
                                        "立即探测"
                                    })
                                </button>
                            </div>
                            <p
                                class="mt-3 mb-0 text-sm text-[#389e0d]"
                                role="status"
                                :hidden=$(probe_success.get().is_empty())
                            >
                                $(probe_success.get())
                            </p>
                            <p
                                class="mt-3 mb-0 text-sm text-[#cf1322]"
                                role="status"
                                :hidden=$(probe_failure.get().is_empty())
                            >
                                $(probe_failure.get())
                            </p>
                        </div>
                    </div>
                </div>
                <footer
                    class="flex justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4"
                >
                    <button
                        class=(BUTTON)
                        type="button"
                        (close)
                        :disabled=$(busy.get())
                    >
                        "取消"
                    </button>
                    <button
                        class=(class!(BUTTON, PRIMARY, "disabled:cursor-not-allowed!"))
                        type="submit"
                        :disabled=$(if busy.get() {
                            true
                        } else if id.get().is_empty() {
                            !bulk_alias_warning.get().is_empty()
                        } else {
                            raw!(
                                "(() => { const saved = ${existing_models}.dehydrate().v; const ownId = String(${id}.get()); const current = String(${alias}.get()).trim() || String(${provider_name}.get()) + '/' + String(${model_id}.get()); return saved.some((record) => record.v.id !== ownId && record.v.alias === current); })()",
                                false,
                            )
                        })
                    >
                        $(if id.get().is_empty() {
                            "导入模型"
                        } else {
                            "保存模型"
                        })
                    </button>
                </footer>
            </form>
        )
    })
}
