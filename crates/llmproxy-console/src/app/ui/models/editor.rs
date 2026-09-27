use super::*;

pub(super) struct Editor {
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
    model_search: Signal<String>,
    model_menu_open: Signal<bool>,
    manual_model_id: Signal<String>,
    manual_open: Signal<bool>,
    selected_models: Signal<String>,
    candidates: Signal<String>,
    candidate_status: Signal<String>,
    chat: Signal<bool>,
    responses: Signal<bool>,
    messages: Signal<bool>,
    supports_chat: Signal<bool>,
    supports_responses: Signal<bool>,
    supports_messages: Signal<bool>,
    probe_protocol: Signal<String>,
    probe_tokens: Signal<String>,
    probe_busy: Signal<bool>,
    probe_success: Signal<String>,
    probe_failure: Signal<String>,
    probe_chat: Signal<bool>,
    probe_responses: Signal<bool>,
    probe_messages: Signal<bool>,
}

impl Editor {
    pub(super) fn new(cx: &Cx) -> Self {
        Self {
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
            model_search: signal(cx, String::new),
            model_menu_open: signal(cx, || false),
            manual_model_id: signal(cx, String::new),
            manual_open: signal(cx, || false),
            selected_models: signal(cx, || "[]".to_owned()),
            candidates: signal(cx, || "[]".to_owned()),
            candidate_status: signal(cx, String::new),
            chat: signal(cx, || false),
            responses: signal(cx, || false),
            messages: signal(cx, || false),
            supports_chat: signal(cx, || false),
            supports_responses: signal(cx, || false),
            supports_messages: signal(cx, || false),
            probe_protocol: signal(cx, String::new),
            probe_tokens: signal(cx, || "1".to_owned()),
            probe_busy: signal(cx, || false),
            probe_success: signal(cx, String::new),
            probe_failure: signal(cx, String::new),
            probe_chat: signal(cx, || false),
            probe_responses: signal(cx, || false),
            probe_messages: signal(cx, || false),
        }
    }
}

pub(super) fn editor_trigger(
    cx: &Cx,
    editor: &Editor,
    model: Option<&ModelMappingView>,
    provider: Option<&ProviderView>,
) -> Attributes {
    let (id, version, alias, provider_id, model_id, chat, responses, messages) =
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
            )
        };
    let Editor {
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
        model_search,
        model_menu_open,
        manual_model_id,
        manual_open,
        selected_models,
        candidates,
        candidate_status,
        chat: selected_chat,
        responses: selected_responses,
        messages: selected_messages,
        supports_chat,
        supports_responses,
        supports_messages,
        probe_protocol,
        probe_tokens,
        probe_busy,
        probe_success,
        probe_failure,
        probe_chat,
        probe_responses,
        probe_messages,
        ..
    } = editor;
    let supported_chat = provider.is_some_and(|provider| provider.paths.openai_chat.is_some());
    let supported_responses =
        provider.is_some_and(|provider| provider.paths.openai_responses.is_some());
    let supported_messages =
        provider.is_some_and(|provider| provider.paths.anthropic_messages.is_some());
    let provider_name = provider
        .map_or("", |provider| provider.name.as_str())
        .to_owned();
    let initial_probe_protocol = if chat {
        Protocol::OpenAiChat.as_str()
    } else if responses {
        Protocol::OpenAiResponses.as_str()
    } else if messages {
        Protocol::AnthropicMessages.as_str()
    } else {
        ""
    };
    attributes! { cx => aria-haspopup="dialog" aria-controls="model-dialog" @click=$(|_event: Event| {
        selected_id.set(id.to_owned());
        selected_version.set(version.to_owned());
        selected_alias.set(alias.to_owned());
        alias_edited.set(!id.is_empty());
        selected_provider.set(provider_id.to_owned());
        selected_provider_name.set(provider_name.to_owned());
        provider_query.set(provider_name.to_owned());
        provider_menu_open.set(false);
        selected_model.set(model_id.to_owned());
        model_search.set("".to_owned());
        model_menu_open.set(false);
        manual_model_id.set("".to_owned());
        manual_open.set(false);
        selected_models.set("[]".to_owned());
        candidates.set("[]".to_owned());
        candidate_status.set("".to_owned());
        selected_chat.set(chat);
        selected_responses.set(responses);
        selected_messages.set(messages);
        supports_chat.set(supported_chat);
        supports_responses.set(supported_responses);
        supports_messages.set(supported_messages);
        probe_protocol.set(initial_probe_protocol.to_owned());
        probe_tokens.set("1".to_owned());
        probe_busy.set(false);
        probe_success.set("".to_owned());
        probe_failure.set("".to_owned());
        probe_chat.set(chat);
        probe_responses.set(responses);
        probe_messages.set(messages);
        error.set("".to_owned());
        open.set(true);
    }) }
}

#[topcoat::view::component]
async fn provider_search(
    cx: &Cx,
    providers: &[ProviderView],
    editor: &Editor,
) -> Result<impl View> {
    let _ = cx;
    let Editor {
        id: editing_id,
        provider_id,
        provider_name,
        provider_query,
        provider_menu_open,
        model_id,
        alias,
        alias_edited,
        chat,
        responses,
        messages,
        supports_chat,
        supports_responses,
        supports_messages,
        selected_models,
        candidates,
        candidate_status,
        model_search,
        model_menu_open,
        manual_model_id,
        manual_open,
        ..
    } = editor;
    let _root_id = "model-provider-select".to_owned();
    let _list_id = "model-provider-options".to_owned();
    let unavailable: Outcome = Err("模型探测请求失败，请重试".into());
    Ok(view! {
        <div id="model-provider-select" class="relative min-w-0" @focusout=$(|_event: Event| {
            let _root_id = _root_id.to_owned();
            raw!("setTimeout(() => { const root = document.getElementById(${_root_id}.dehydrate()); if (root && !root.contains(document.activeElement)) ${provider_menu_open}.set(cx.hydrate(false)); }, 0)", ());
        })>
            <input id="model-provider-input" class="w-full" type="search" role="combobox" aria-label="搜索并选择 Provider" aria-autocomplete="list" aria-controls="model-provider-options" :aria-expanded=$(provider_menu_open.get()) :value=$(provider_query.get()) placeholder="搜索已启用 Provider" autocomplete="off" required=""
                @focus=$(|_event: Event| provider_menu_open.set(true))
                @input=$(|event: Event| {
                    provider_query.set(event.target.value);
                    provider_menu_open.set(true);
                })
                @keydown=$(|_event: Event| {
                    let _list_id = _list_id.to_owned();
                    raw!("{ const key = ${_event}.key.dehydrate(); if (key === 'ArrowDown' || key === 'Enter') { if (key === 'Enter') ${_event}.prevent_default(); const query = ${_event}.target.value.dehydrate().toLowerCase(); const first = [...(document.getElementById(${_list_id}.dehydrate())?.querySelectorAll('button[role=option]') ?? [])].find(option => option.textContent.toLowerCase().includes(query)); if (first) { if (key === 'ArrowDown') ${_event}.prevent_default(); if (key === 'Enter') first.click(); else first.focus(); } } else if (key === 'Escape') { ${provider_menu_open}.set(cx.hydrate(false)); } }", ());
                })>
            <div id="model-provider-options" class="absolute z-30 mt-1 max-h-48 w-full overflow-y-auto rounded-md border border-control-border bg-white p-1 shadow-[0_6px_16px_rgba(0,0,0,0.08)]" role="listbox" aria-label="已启用 Provider" :hidden=$(!provider_menu_open.get())>
                #[key(provider.id)] for provider in providers {
                    let option_id = provider.id.to_string();
                    let option_name = provider.name.clone();
                    let has_chat = provider.paths.openai_chat.is_some();
                    let has_responses = provider.paths.openai_responses.is_some();
                    let has_messages = provider.paths.anthropic_messages.is_some();
                    let only_one = provider.paths.supported().len() == 1;
                    let default_chat = only_one && has_chat;
                    let default_responses = only_one && has_responses;
                    let default_messages = only_one && has_messages;
                    let unavailable = unavailable.clone();
                    let confirm_id = format!("switch-model-provider-{}", provider.id);
                    let confirm_title = format!("切换到「{}」？未保存的模型会清空。", provider.name);
                    let option_class = "block w-full rounded-md border-0! bg-transparent! px-3 py-2 text-left text-sm leading-5 text-heading shadow-none! hover:bg-primary-soft! focus:bg-primary-soft! focus:outline-none aria-selected:text-primary data-[filtered]:hidden";
                    let choose = attributes! { cx => @click=$(async |_event: Event| {
                            provider_id.set(option_id.to_owned());
                            provider_name.set(option_name.to_owned());
                            provider_query.set(option_name.to_owned());
                            provider_menu_open.set(false);
                            model_id.set("".to_owned());
                            if !alias_edited.get() { alias.set("".to_owned()); }
                            chat.set(default_chat);
                            responses.set(default_responses);
                            messages.set(default_messages);
                            supports_chat.set(has_chat);
                            supports_responses.set(has_responses);
                            supports_messages.set(has_messages);
                            selected_models.set("[]".to_owned());
                            candidates.set("[]".to_owned());
                            candidate_status.set("".to_owned());
                            model_search.set("".to_owned());
                            model_menu_open.set(false);
                            manual_model_id.set("".to_owned());
                            manual_open.set(false);
                            let creating = editing_id.get().is_empty();
                            if creating { candidate_status.set("loading".to_owned()); }
                            let result = raw!("await Promise.resolve(${creating}.dehydrate() ? ${load_model_candidates}.call(${option_id}) : ${unavailable}).catch(() => ${unavailable})", unavailable.clone());
                            if creating {
                                if provider_id.get() == option_id {
                                    if result.is_ok() { candidates.set(result.unwrap()); candidate_status.set("".to_owned()); }
                                    else { candidate_status.set(result.unwrap_err()); }
                                }
                            }
                        }) };
                    <button class=(option_class) type="button" role="option" :aria-selected=$(provider_id.get() == option_id) :data-filtered=$(if provider_query.get().is_empty() { false } else { raw!("!${option_name}.dehydrate().toLowerCase().includes(${provider_query}.get().dehydrate().toLowerCase())", false) }) :hidden=$(if selected_models.get() == "[]" { false } else { provider_id.get() != option_id }) (choose.clone())>(provider.name.as_str())</button>
                    <button class=(option_class) type="button" role="option" :aria-selected=$(provider_id.get() == option_id) :data-filtered=$(if provider_query.get().is_empty() { false } else { raw!("!${option_name}.dehydrate().toLowerCase().includes(${provider_query}.get().dehydrate().toLowerCase())", false) }) :hidden=$(if selected_models.get() == "[]" { true } else { provider_id.get() == option_id }) (popconfirm_trigger_attributes(cx, &confirm_id))>(provider.name.as_str())</button>
                    popconfirm(id: confirm_id.as_str(), title: confirm_title.as_str(), language: UiLanguage::ChineseSimplified,
                        attrs: attributes! { cx => @click=$(|_event: Event| provider_query.set(provider_name.get())) },
                        <button class="gr-button gr-button-danger" type="button" (choose)>"确认切换"</button>
                    )
                }
            </div>
        </div>
    })
}

#[topcoat::view::component]
async fn draft_model_row(
    cx: &Cx,
    model_id: &str,
    index: usize,
    provider_name: &str,
    selected: Signal<String>,
    supports_chat: bool,
    supports_responses: bool,
    supports_messages: bool,
    default_chat: bool,
    default_responses: bool,
    default_messages: bool,
) -> Result<impl View> {
    let alias = signal(cx, || format!("{provider_name}/{model_id}"));
    let chat = signal(cx, || default_chat);
    let responses = signal(cx, || default_responses);
    let messages = signal(cx, || default_messages);
    let row_id = model_id.to_owned();
    Ok(view! {
        <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)_minmax(0,1.3fr)_32px] items-start gap-3 py-3 max-[760px]:grid-cols-[minmax(0,1fr)_32px]">
            <input type="hidden" name="model_id" value=(model_id)>
            <div class="min-w-0 pt-2 text-sm break-all text-heading max-[760px]:col-span-1 max-[760px]:pt-0"><span class="hidden text-xs text-secondary max-[760px]:block">"模型 ID"</span>(model_id)</div>
            <div class="min-w-0 max-[760px]:col-span-1 max-[760px]:row-start-2"><label class="hidden text-xs text-secondary max-[760px]:mb-1 max-[760px]:block" for=(format!("alias-{index}"))>"客户端别名"</label><input id=(format!("alias-{index}")) name=(format!("alias-{index}")) class="w-full" type="text" :value=$(alias.get()) maxlength="200" aria-label=(format!("{model_id} 的客户端别名")) @input=$(|event: Event| alias.set(event.target.value))></div>
            <div class="flex min-w-0 flex-wrap gap-x-3 gap-y-1 pt-2 text-xs text-heading max-[760px]:col-span-1 max-[760px]:row-start-3 max-[760px]:pt-0">
                <span class="hidden w-full text-xs text-secondary max-[760px]:block">"支持协议"</span>
                if supports_chat { <label class="inline-flex items-center gap-1"><input type="checkbox" name=(format!("chat-{index}")) :checked=$(chat.get()) @change=$(|event: Event| chat.set(event.target.checked))>"Chat"</label> }
                if supports_responses { <label class="inline-flex items-center gap-1"><input type="checkbox" name=(format!("responses-{index}")) :checked=$(responses.get()) @change=$(|event: Event| responses.set(event.target.checked))>"Responses"</label> }
                if supports_messages { <label class="inline-flex items-center gap-1"><input type="checkbox" name=(format!("messages-{index}")) :checked=$(messages.get()) @change=$(|event: Event| messages.set(event.target.checked))>"Messages"</label> }
            </div>
            <button class="inline-flex size-8 shrink-0 items-center justify-center self-center rounded-full border border-control-border bg-white p-0 text-muted shadow-sm transition-colors duration-150 hover:border-[#ffccc7] hover:bg-[#fff2f0] hover:text-[#cf1322] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#91caff] max-[760px]:col-start-2 max-[760px]:row-start-1" type="button" aria-label=(format!("移除 {model_id}")) title="移除模型" @click=$(async |_event: Event| {
                selected.set(remove_draft_model(selected.get(), row_id.to_owned()).await);
            })>icon(data: CLOSE_OUTLINED, size: 14)</button>
        </div>
    })
}

#[shard("/ui/_topcoat/runtime/shards/model-draft")]
pub async fn model_draft(
    cx: &Cx,
    provider_id: Signal<String>,
    provider_name: Signal<String>,
    candidates: Signal<String>,
    candidate_status: Signal<String>,
    selected: Signal<String>,
    search: Signal<String>,
    menu_open: Signal<bool>,
    manual_model_id: Signal<String>,
    manual_open: Signal<bool>,
    supports_chat: Signal<bool>,
    supports_responses: Signal<bool>,
    supports_messages: Signal<bool>,
) -> Result<impl View> {
    let _ = cx;
    let available: Vec<String> = serde_json::from_str(&candidates.get()).unwrap_or_default();
    let discovered_count = available.len();
    let selected_ids: Vec<String> = serde_json::from_str(&selected.get()).unwrap_or_default();
    let mut chosen = HashSet::new();
    let rows: Vec<_> = selected_ids
        .into_iter()
        .filter(|id| chosen.insert(id.clone()))
        .collect();
    let mut seen = HashSet::new();
    let mut options: Vec<SearchOption> = available
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|id| SearchOption {
            value: id.clone(),
            label: id,
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
        supports_chat.get(),
        supports_responses.get(),
        supports_messages.get(),
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count()
        == 1;
    let provider_key = provider_id.get();
    let provider_label = provider_name.get();
    let has_chat = supports_chat.get();
    let has_responses = supports_responses.get();
    let has_messages = supports_messages.get();
    Ok(view! {
            <div class="min-w-0">
                <label class="mb-2 block text-sm font-semibold text-heading" for="model-select-input">"选择上游模型"<span class="ml-1 text-[#ff4d4f]">"*"</span></label>
                if options.is_empty() {
                    <input id="model-select-input" class="w-full" type="search" placeholder="搜索上游模型" disabled="">
                } else {
                    search_multi_select(id: "model-select", options: &options, selected: &selected, search: &search, open: &menu_open, label: Some("上游模型"), placeholder: Some("搜索上游模型"), language: UiLanguage::ChineseSimplified)
                }
                <div class="mt-2 flex flex-wrap items-center justify-between gap-2">
                    <p class="m-0 text-xs text-secondary" role="status">
                        if provider_id.get().is_empty() { "先选择 Provider，再选择或手动添加模型。" }
                        else if candidate_status.get() == "loading" { "正在探测模型列表，也可以先手动添加。" }
                        else if discovered_count == 0 { "模型列表暂无可选项，也可以手动添加。" }
                        else { (format!("探测到 {discovered_count} 个模型，可搜索并一次选择多个。")) }
                    </p>
                    <button class="shrink-0 border-0 bg-transparent p-0 text-xs font-medium text-primary hover:underline" type="button" :hidden=$(provider_id.get().is_empty()) @click=$(|_event: Event| manual_open.set(!manual_open.get()))>$(if manual_open.get() { "收起手动添加" } else { "+ 手动添加模型 ID" })</button>
                </div>
                <p class="mt-2 mb-0 text-xs text-[#cf1322]" role="alert" :hidden=$(if candidate_status.get() == "loading" { true } else { candidate_status.get().is_empty() })>$(candidate_status.get())</p>
                <div class="mt-3 flex items-center gap-2 rounded-md border border-border bg-surface/40 p-2 max-[480px]:flex-wrap" :hidden=$(!manual_open.get())>
                    <input id="manual-model-id" class="min-w-0 flex-1 max-[480px]:basis-full" type="text" aria-label="手动输入上游模型 ID" placeholder="输入上游模型 ID" maxlength="200" autocomplete="off" :value=$(manual_model_id.get()) @input=$(|event: Event| manual_model_id.set(event.target.value)) @keydown=$(|_event: Event| {
                        raw!("if (${_event}.key.dehydrate() === 'Enter') { ${_event}.prevent_default(); document.getElementById('manual-model-add')?.click(); }", ());
                    })>
                    <button id="manual-model-add" class=(BUTTON) type="button" :disabled=$(manual_model_id.get().trim().is_empty()) @click=$(async |_event: Event| {
                        selected.set(add_draft_model(selected.get(), manual_model_id.get()).await);
                        manual_model_id.set("".to_owned());
                        manual_open.set(false);
                    })>"添加"</button>
                </div>
                <p class="mt-2 mb-0 text-xs text-secondary" :hidden=$(!manual_open.get())>"手动添加的模型不会经过列表校验；保存后可在编辑页探测可用性。"</p>
            </div>
            if !rows.is_empty() {
                <div class="col-span-full mt-2">
                    <h3 class="mb-3 text-sm font-semibold text-heading">(format!("待导入模型（{}）", rows.len()))</h3>
                    <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)_minmax(0,1.3fr)_32px] gap-3 border-b border-border pb-2 text-xs font-medium text-secondary max-[760px]:hidden">
                        <span>"模型 ID"</span><span>"客户端别名"</span><span>"支持协议"</span><span class="sr-only">"操作"</span>
                    </div>
                    <div class="divide-y divide-border">
                        #[key((provider_key.as_str(), row_id.as_str()))] for (index, row_id) in rows.iter().enumerate() {
                            draft_model_row(
                                model_id: row_id.as_str(),
                                index: index,
                                provider_name: provider_label.as_str(),
                                selected: selected.clone(),
                                supports_chat: has_chat,
                                supports_responses: has_responses,
                                supports_messages: has_messages,
                                default_chat: only_one && has_chat,
                                default_responses: only_one && has_responses,
                                default_messages: only_one && has_messages,
                            )
                        }
                    </div>
                </div>
            }
    })
}

#[topcoat::view::component]
pub(super) async fn model_editor(
    cx: &Cx,
    editor: &Editor,
    providers: &[ProviderView],
    csrf: &str,
    success: &Signal<String>,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let Editor {
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
        chat,
        responses,
        messages,
        supports_chat,
        supports_responses,
        supports_messages,
        candidates,
        selected_models,
        model_search,
        model_menu_open,
        manual_model_id,
        manual_open,
        candidate_status,
        probe_protocol,
        probe_tokens,
        probe_busy,
        probe_success,
        probe_failure,
        probe_chat,
        probe_responses,
        probe_messages,
        ..
    } = editor;
    let close = native_dialog_close_attributes(cx, "model-dialog");
    let unavailable: Outcome = Err("保存请求失败，请刷新后重试".into());
    let probe_unavailable: Outcome = Err("探测请求失败，请重试".into());
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new("model-dialog", "模型配置"),
            open: Some(open), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="w-[min(960px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:px-6 [&_.gr-native-dialog-header]:py-4" },
            <form id="model-editor-form" class="m-0 flex min-h-0 max-h-[calc(100dvh_-_48px)] flex-col" @submit=$(async |event: Event| {
                event.prevent_default();
                if busy.get() { return; }
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
                let result = raw!("await Promise.resolve(${id}.get().dehydrate() ? ${save_model}.call(${csrf}, ${id}.get(), ${version}.get(), ${alias}.get(), ${provider_id}.get(), ${model_id}.get(), ${chat}.get(), ${responses}.get(), ${messages}.get()) : (() => { const form = new FormData(document.getElementById('model-editor-form')); const rows = form.getAll('model_id').map((model_id, index) => ({model_id, alias: form.get('alias-' + index) || '', chat: form.has('chat-' + index), responses: form.has('responses-' + index), messages: form.has('messages-' + index)})); return ${save_models}.call(${csrf}, ${provider_id}.get(), cx.hydrate(JSON.stringify(rows))); })()).catch(() => ${unavailable})", unavailable.clone());
                busy.set(false);
                if result.is_ok() { open.set(false); success.set(result.unwrap()); refresh.increment(); }
                else { error.set(result.unwrap_err()); }
            })>
                <div class="min-h-[min(320px,calc(100dvh_-_180px))] overflow-y-auto p-6">
                    <div class="mb-5 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]" role="alert" :hidden=$(error.get().is_empty())>$(error.get())</div>
                    <div class="grid grid-cols-2 items-start gap-x-5 gap-y-4 max-[760px]:grid-cols-1">
                        <div class="min-w-0">
                            <label class="mb-2 block text-sm font-semibold" for="model-provider-input">"选择已启用 Provider"<span class="ml-1 text-[#ff4d4f]">"*"</span></label>
                            provider_search(providers: providers, editor: editor)
                        </div>
                        <div class="contents" :hidden=$(!id.get().is_empty())>
                            model_draft(provider_id: provider_id.clone(), provider_name: provider_name.clone(), candidates: candidates.clone(), candidate_status: candidate_status.clone(), selected: selected_models.clone(), search: model_search.clone(), menu_open: model_menu_open.clone(), manual_model_id: manual_model_id.clone(), manual_open: manual_open.clone(), supports_chat: supports_chat.clone(), supports_responses: supports_responses.clone(), supports_messages: supports_messages.clone())
                        </div>
                    </div>
                    <div :hidden=$(id.get().is_empty())>
                        <div class="mt-6">form_field(config: FormFieldConfig::new("model-id", "上游模型 ID").required(),
                            <input id="model-id" list="model-candidates" autocomplete="off" :value=$(model_id.get()) @input=$(|event: Event| {
                                model_id.set(event.target.value);
                                if !alias_edited.get() {
                                    alias.set("".to_owned());
                                    if !model_id.get().is_empty() {
                                        alias.set(provider_name.get());
                                        alias.push_str("/");
                                        alias.push_str(model_id.get());
                                    }
                                }
                            }) placeholder="搜索探测结果，或手动输入模型 ID" :disabled=$(provider_id.get().is_empty())>
                        )</div>
                        model_candidates(provider_id: $(provider_id.get()), open: $(if id.get().is_empty() { false } else { open.get() }))
                        <div class="mt-6">form_field(config: FormFieldConfig::new("model-alias", "客户端别名"),
                            <input id="model-alias" :value=$(alias.get()) @input=$(|event: Event| { alias.set(event.target.value); alias_edited.set(true); }) placeholder="Provider名称/模型ID" maxlength="200">
                        )</div>
                        <h3 class="mt-6 mb-3 text-sm font-semibold">"选择可用协议（至少一个）"</h3>
                        <div class="flex flex-wrap gap-4 text-sm text-heading">
                            <label class="flex items-center gap-2" :hidden=$(!supports_chat.get())><input type="checkbox" :checked=$(chat.get()) @change=$(|event: Event| chat.set(event.target.checked))>"Chat"</label>
                            <label class="flex items-center gap-2" :hidden=$(!supports_responses.get())><input type="checkbox" :checked=$(responses.get()) @change=$(|event: Event| responses.set(event.target.checked))>"Responses"</label>
                            <label class="flex items-center gap-2" :hidden=$(!supports_messages.get())><input type="checkbox" :checked=$(messages.get()) @change=$(|event: Event| messages.set(event.target.checked))>"Messages"</label>
                        </div>
                        <div class="mt-6 rounded-lg border border-border bg-surface/40 p-4">
                            <h3 class="m-0 text-sm font-semibold text-heading">"可用性探测"</h3>
                            <p class="mt-1 mb-4 text-xs text-secondary">"对已保存的模型发一次极短请求。按已保存的协议和上游路径探测，不修改模型配置。"</p>
                            <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-end gap-3 max-[640px]:grid-cols-1">
                                <label class="min-w-0 text-xs font-medium text-heading">"探测协议"
                                    <select class="mt-2 w-full" :value=$(probe_protocol.get()) @change=$(|event: Event| { probe_protocol.set(event.target.value); probe_success.set("".to_owned()); probe_failure.set("".to_owned()); })>
                                        <option value="openai_chat" :disabled=$(!probe_chat.get()) :hidden=$(!probe_chat.get())>"Chat"</option>
                                        <option value="openai_responses" :disabled=$(!probe_responses.get()) :hidden=$(!probe_responses.get())>"Responses"</option>
                                        <option value="anthropic_messages" :disabled=$(!probe_messages.get()) :hidden=$(!probe_messages.get())>"Messages"</option>
                                    </select>
                                </label>
                                <label class="min-w-0 text-xs font-medium text-heading">"输出上限（token）"
                                    <input class="mt-2 w-full" type="number" min="1" max="1024" step="1" :value=$(probe_tokens.get()) @input=$(|event: Event| { probe_tokens.set(event.target.value); probe_success.set("".to_owned()); probe_failure.set("".to_owned()); })>
                                </label>
                                <button class=(class!(BUTTON, "h-10")) type="button" :disabled=$(probe_busy.get()) @click=$(async |_event: Event| {
                                    if busy.get() { return; }
                                    if probe_busy.get() { return; }
                                    busy.set(true);
                                    probe_busy.set(true);
                                    probe_success.set("".to_owned());
                                    probe_failure.set("".to_owned());
                                    let result = raw!("await Promise.resolve(${probe_saved_model}.call(${csrf}, ${id}.get(), ${probe_protocol}.get(), ${probe_tokens}.get())).catch(() => ${probe_unavailable})", probe_unavailable.clone());
                                    probe_busy.set(false);
                                    busy.set(false);
                                    if result.is_ok() { probe_success.set(result.unwrap()); }
                                    else { probe_failure.set(result.unwrap_err()); }
                                })>$(if probe_busy.get() { "探测中…" } else { "立即探测" })</button>
                            </div>
                            <p class="mt-3 mb-0 text-sm text-[#389e0d]" role="status" :hidden=$(probe_success.get().is_empty())>$(probe_success.get())</p>
                            <p class="mt-3 mb-0 text-sm text-[#cf1322]" role="status" :hidden=$(probe_failure.get().is_empty())>$(probe_failure.get())</p>
                        </div>
                    </div>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4"><button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button><button class=(class!(BUTTON, PRIMARY)) type="submit" :disabled=$(busy.get())>$(if id.get().is_empty() { "导入模型" } else { "保存模型" })</button></footer>
            </form>
        )
    })
}
