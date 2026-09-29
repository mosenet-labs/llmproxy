use super::*;

// Business state stays in Topcoat. IDs and versions remain strings in the
// browser so database integer values are never rounded through JavaScript f64.
pub(super) struct EditorSignals {
    open: Signal<bool>,
    title: Signal<String>,
    busy: Signal<bool>,
    advanced: Signal<bool>,
    error: Signal<String>,
    id: Signal<String>,
    version: Signal<String>,
    name: Signal<String>,
    openai_chat: Signal<bool>,
    openai_chat_path: Signal<String>,
    openai_responses: Signal<bool>,
    openai_responses_path: Signal<String>,
    anthropic_messages: Signal<bool>,
    anthropic_messages_path: Signal<String>,
    gemini: Signal<bool>,
    gemini_path: Signal<String>,
    upstream_url: Signal<String>,
    enabled: Signal<bool>,
    api_key: Signal<String>,
    models_path: Signal<String>,
    models_protocol: Signal<String>,
    models_probe_status: Signal<String>,
    models_probe_message: Signal<String>,
    anthropic_version: Signal<String>,
    messages_auth: Signal<String>,
    connect_timeout: Signal<String>,
    read_timeout: Signal<String>,
    write_timeout: Signal<String>,
}

impl EditorSignals {
    pub(super) fn new(cx: &Cx, input: &ProviderForm, error: Option<&str>, open: bool) -> Self {
        Self {
            open: signal(cx, || open),
            title: signal(cx, || {
                if input.id.is_some() {
                    "编辑 Provider"
                } else {
                    "新建 Provider"
                }
                .to_owned()
            }),
            busy: signal(cx, || false),
            advanced: signal(cx, || error.is_some()),
            error: signal(cx, || error.unwrap_or_default().to_owned()),
            id: signal(cx, || input.id.clone().unwrap_or_default()),
            version: signal(cx, || input.version.clone().unwrap_or_default()),
            name: signal(cx, || input.name.clone()),
            openai_chat: signal(cx, || input.openai_chat),
            openai_chat_path: signal(cx, || input.openai_chat_path.clone()),
            openai_responses: signal(cx, || input.openai_responses),
            openai_responses_path: signal(cx, || input.openai_responses_path.clone()),
            anthropic_messages: signal(cx, || input.anthropic_messages),
            anthropic_messages_path: signal(cx, || input.anthropic_messages_path.clone()),
            gemini: signal(cx, || input.gemini),
            gemini_path: signal(cx, || input.gemini_path.clone()),
            upstream_url: signal(cx, || input.upstream_url.clone()),
            enabled: signal(cx, || input.enabled),
            // Never initialize browser state from a submitted or stored secret.
            api_key: signal(cx, String::new),
            models_path: signal(cx, || input.models_path.clone()),
            models_protocol: signal(cx, || input.models_protocol.clone()),
            models_probe_status: signal(cx, || input.models_probe_status.clone()),
            models_probe_message: signal(cx, String::new),
            anthropic_version: signal(cx, || input.anthropic_version.clone()),
            messages_auth: signal(cx, || input.messages_auth.clone()),
            connect_timeout: signal(cx, || input.connect_timeout_ms.clone()),
            read_timeout: signal(cx, || input.read_timeout_ms.clone()),
            write_timeout: signal(cx, || input.write_timeout_ms.clone()),
        }
    }
}

pub(super) fn editor_trigger(cx: &Cx, editor: &EditorSignals, input: ProviderForm) -> Attributes {
    let EditorSignals {
        open,
        title,
        busy,
        advanced,
        error,
        id,
        version,
        name,
        openai_chat,
        openai_chat_path,
        openai_responses,
        openai_responses_path,
        anthropic_messages,
        anthropic_messages_path,
        gemini,
        gemini_path,
        upstream_url,
        enabled,
        api_key,
        models_path,
        models_protocol,
        models_probe_status,
        models_probe_message,
        anthropic_version,
        messages_auth,
        connect_timeout,
        read_timeout,
        write_timeout,
    } = editor;
    let initial_title = if input.id.is_some() {
        "编辑 Provider"
    } else {
        "新建 Provider"
    }
    .to_owned();
    let initial_id = input.id.unwrap_or_default();
    let initial_version = input.version.unwrap_or_default();
    let initial_name = input.name;
    let initial_openai_chat = input.openai_chat;
    let initial_openai_chat_path = input.openai_chat_path;
    let initial_openai_responses = input.openai_responses;
    let initial_openai_responses_path = input.openai_responses_path;
    let initial_anthropic_messages = input.anthropic_messages;
    let initial_anthropic_messages_path = input.anthropic_messages_path;
    let initial_gemini = input.gemini;
    let initial_gemini_path = input.gemini_path;
    let initial_upstream_url = input.upstream_url;
    let initial_enabled = input.enabled;
    let initial_anthropic = input.anthropic_version;
    let initial_messages_auth = input.messages_auth;
    let initial_models_path = input.models_path;
    let initial_models_protocol = input.models_protocol;
    let initial_models_probe_status = input.models_probe_status;
    let initial_connect = input.connect_timeout_ms;
    let initial_read = input.read_timeout_ms;
    let initial_write = input.write_timeout_ms;
    attributes! { cx =>
        aria-haspopup="dialog" aria-controls="provider-dialog"
        @click=$(|_event: Event| {
            id.set(initial_id.to_owned());
            version.set(initial_version.to_owned());
            title.set(initial_title.to_owned());
            name.set(initial_name.to_owned());
            openai_chat.set(initial_openai_chat);
            openai_chat_path.set(initial_openai_chat_path.to_owned());
            openai_responses.set(initial_openai_responses);
            openai_responses_path.set(initial_openai_responses_path.to_owned());
            anthropic_messages.set(initial_anthropic_messages);
            anthropic_messages_path.set(initial_anthropic_messages_path.to_owned());
            gemini.set(initial_gemini);
            gemini_path.set(initial_gemini_path.to_owned());
            upstream_url.set(initial_upstream_url.to_owned());
            enabled.set(initial_enabled);
            anthropic_version.set(initial_anthropic.to_owned());
            messages_auth.set(initial_messages_auth.to_owned());
            models_path.set(initial_models_path.to_owned());
            models_protocol.set(initial_models_protocol.to_owned());
            models_probe_status.set(initial_models_probe_status.to_owned());
            models_probe_message.set("".to_owned());
            connect_timeout.set(initial_connect.to_owned());
            read_timeout.set(initial_read.to_owned());
            write_timeout.set(initial_write.to_owned());
            api_key.set("".to_owned());
            error.set("".to_owned());
            advanced.set(false);
            busy.set(false);
            open.set(true);
        })
    }
}

#[component]
pub(super) async fn provider_editor(
    cx: &Cx,
    editor: &EditorSignals,
    controls: &ListSignals,
) -> Result<impl View> {
    let csrf = &app_context::<AppState>(cx).csrf;
    let EditorSignals {
        open,
        title,
        busy,
        advanced,
        error,
        id,
        version,
        name,
        openai_chat,
        openai_chat_path,
        openai_responses,
        openai_responses_path,
        anthropic_messages,
        anthropic_messages_path,
        gemini,
        gemini_path,
        upstream_url,
        enabled,
        api_key,
        models_path,
        models_protocol,
        models_probe_status,
        models_probe_message,
        anthropic_version,
        messages_auth,
        connect_timeout,
        read_timeout,
        write_timeout,
    } = editor;
    let close = native_dialog_close_attributes(cx, "provider-dialog");
    let success = &controls.success;
    let failure = &controls.failure;
    let refresh = &controls.refresh;
    let unavailable: Outcome = Err("保存请求失败或结果未确认，请检查列表状态后重试".to_owned());
    let probe_unavailable: Outcome = Err("模型探测请求失败，请稍后重试".to_owned());
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new("provider-dialog", "Provider 配置"),
            open: Some(open), title: Some(title), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { cx => class="w-[min(920px,calc(100%_-_32px))]! max-[640px]:w-[calc(100%_-_24px)]! max-[640px]:max-h-[calc(100dvh_-_24px)]! [&_.gr-native-dialog-header]:px-6 [&_.gr-native-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg max-[640px]:[&_.gr-native-dialog-header]:px-4 max-[640px]:[&_.gr-native-dialog-header]:py-4" @close=$(|_event: Event| api_key.set("".to_owned())) },
            <form id="provider-editor-form" class="m-0 flex min-h-0 flex-col" action="/ui/providers/save" method="post" autocomplete="off"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    busy.set(true);
                    error.set("".to_owned());
                    success.set("".to_owned());
                    failure.set("".to_owned());
                    // Topcoat 0.9 cannot construct a form value in runtime
                    // expressions and procedure calls accept at most 12 args.
                    let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById('provider-editor-form'))).toString())", String::new());
                    let result = raw!("await Promise.resolve(${save_provider}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
                    api_key.set("".to_owned());
                    busy.set(false);
                    if result.is_ok() {
                        open.set(false);
                        success.set(result.unwrap());
                        refresh.increment();
                    } else {
                        error.set(result.unwrap_err());
                        advanced.set(true);
                    }
                })>
                <input type="hidden" name="csrf" value=(csrf.as_str())>
                <input type="hidden" name="id" :value=$(id.get()) :disabled=$(id.get().is_empty())>
                <input type="hidden" name="version" :value=$(version.get()) :disabled=$(id.get().is_empty())>
                <div class="min-h-0 overflow-y-auto overscroll-contain p-6 max-[640px]:p-4">
                    <div class="mb-5 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm leading-relaxed text-[#cf1322] [&_strong]:mb-1 [&_strong]:block [&_strong]:font-semibold [&_small]:mt-1.5 [&_small]:block [&_small]:text-[13px] [&_small]:text-[#a61d24]" role="alert" :hidden=$(error.get().is_empty())><strong>"保存失败"</strong><span>$(error.get())</span><small>"API Key 不会回显；如需新增或更换凭据，请重新输入。"</small></div>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-6 [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"基本信息"</h3>
                        <div class=(FIELDS_GRID)>
                            form_field(config: FormFieldConfig::new("name", "Provider 名称").required(), <input id="name" name="name" :value=$(name.get()) @input=$(|event: Event| name.set(event.target.value)) placeholder="例如：OpenAI · Production" maxlength="80" required="" autofocus="">)
                        </div>
                        <div class="mt-5 flex items-start gap-2 text-sm leading-[22px] text-heading [&_small]:ml-3 [&_small]:inline [&_small]:text-[13px] [&_small]:text-muted max-[640px]:[&_small]:ml-0 max-[640px]:[&_small]:block"><input type="hidden" name="enabled" :value=$(if enabled.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if enabled.get() { "true" } else { "false" }) @click=$(|_event: Event| enabled.toggle())><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span><span>"启用此 Provider"</span></button><small>"启用后可在 Models 中添加模型。"</small></div>
                    </section>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-6 [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"接口协议与上游路径"</h3>
                        <p class=(FIELD_HINT)>"至少选择一个协议。路径只填写上游接口路径。"</p>
                        <div class="mt-4 grid gap-4">
                            <div class="grid grid-cols-[190px_1fr] items-center gap-3 max-[640px]:grid-cols-1"><input type="hidden" name="openai_chat" :value=$(if openai_chat.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if openai_chat.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !openai_chat.get(); openai_chat.set(selected); if !selected { if models_protocol.get() == "openai_chat" { models_protocol.set(if openai_responses.get() { "openai_responses" } else if anthropic_messages.get() { "anthropic_messages" } else if gemini.get() { "gemini" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("openai_chat".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"OpenAIChat"</button><input class="w-full" name="openai_chat_path" aria-label="OpenAIChat 上游路径" :value=$(openai_chat_path.get()) @input=$(|event: Event| openai_chat_path.set(event.target.value)) :disabled=$(!openai_chat.get()) :required=$(openai_chat.get())></div>
                            <div class="grid grid-cols-[190px_1fr] items-center gap-3 max-[640px]:grid-cols-1"><input type="hidden" name="openai_responses" :value=$(if openai_responses.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if openai_responses.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !openai_responses.get(); openai_responses.set(selected); if !selected { if models_protocol.get() == "openai_responses" { models_protocol.set(if openai_chat.get() { "openai_chat" } else if anthropic_messages.get() { "anthropic_messages" } else if gemini.get() { "gemini" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("openai_responses".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"OpenAIResponses"</button><input class="w-full" name="openai_responses_path" aria-label="OpenAIResponses 上游路径" :value=$(openai_responses_path.get()) @input=$(|event: Event| openai_responses_path.set(event.target.value)) :disabled=$(!openai_responses.get()) :required=$(openai_responses.get())></div>
                            <div class="grid grid-cols-[190px_minmax(0,1fr)] items-center gap-3 max-[640px]:grid-cols-1">
                                <input type="hidden" name="anthropic_messages" :value=$(if anthropic_messages.get() { "true" } else { "false" })>
                                <input type="hidden" name="messages_auth" :value=$(messages_auth.get())>
                                <button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if anthropic_messages.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !anthropic_messages.get(); anthropic_messages.set(selected); if !selected { if models_protocol.get() == "anthropic_messages" { models_protocol.set(if openai_chat.get() { "openai_chat" } else if openai_responses.get() { "openai_responses" } else if gemini.get() { "gemini" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("anthropic_messages".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"AnthropicMessages"</button>
                                <div class="grid min-w-0 grid-cols-[minmax(0,1fr)_190px_190px] items-center gap-3 max-[900px]:grid-cols-2 max-[480px]:grid-cols-1">
                                    <input class="min-w-0 w-full max-[900px]:col-span-2 max-[480px]:col-span-1" name="anthropic_messages_path" aria-label="AnthropicMessages 上游路径" :value=$(anthropic_messages_path.get()) @input=$(|event: Event| anthropic_messages_path.set(event.target.value)) :disabled=$(!anthropic_messages.get()) :required=$(anthropic_messages.get())>
                                    <div class="flex min-w-0 items-center gap-2" :hidden=$(!anthropic_messages.get())><label class="shrink-0 whitespace-nowrap text-sm text-heading" for="anthropic-version">"API版本"</label><input class="min-w-0 flex-1" id="anthropic-version" name="anthropic_version" :value=$(anthropic_version.get()) @input=$(|event: Event| { anthropic_version.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) placeholder="2023-06-01" :disabled=$(!anthropic_messages.get())></div>
                                    <div class="flex min-w-0 items-center gap-2" :hidden=$(!anthropic_messages.get())><label class="shrink-0 whitespace-nowrap text-sm text-heading" for="messages-auth">"鉴权"</label>select(attrs: attributes! { cx => id="messages-auth" aria-label="Messages 鉴权方式" class="min-w-0 flex-1 [&>select]:whitespace-nowrap" :value=$(messages_auth.get()) :disabled=$(!anthropic_messages.get()) @change=$(|event: Event| { messages_auth.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) }, <option value="x-api-key">"x-api-key"</option><option value="bearer">"Bearer"</option>)</div>
                                </div>
                            </div>
                            <div class="grid grid-cols-[190px_1fr] items-center gap-3 max-[640px]:grid-cols-1">
                                <input type="hidden" name="gemini" :value=$(if gemini.get() { "true" } else { "false" })>
                                <button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if gemini.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !gemini.get(); gemini.set(selected); if !selected { if models_protocol.get() == "gemini" { models_protocol.set(if openai_chat.get() { "openai_chat" } else if openai_responses.get() { "openai_responses" } else if anthropic_messages.get() { "anthropic_messages" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("gemini".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"Gemini"</button>
                                <input class="w-full" name="gemini_path" aria-label="Gemini 上游模型路径前缀" :value=$(gemini_path.get()) @input=$(|event: Event| gemini_path.set(event.target.value)) :disabled=$(!gemini.get()) :required=$(gemini.get())>
                            </div>
                        </div>
                    </section>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-6 [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"连接与凭据"</h3>
                        <div class=(FIELDS_GRID)>
                            <div class="col-span-full">form_field(config: FormFieldConfig::new("upstream-url", "上游地址").required(), <input id="upstream-url" name="upstream_url" type="url" :value=$(upstream_url.get()) @input=$(|event: Event| { upstream_url.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) placeholder="https://api.deepseek.com" required="" aria-describedby="connection-help">)</div>
                        </div>
                        <p class=(FIELD_HINT) id="connection-help">"例如 https://api.deepseek.com；本地服务可填写 http://127.0.0.1:11434。"</p>
                        <div class=(class!(FIELDS_GRID, "mt-5"))>
                            <div class="col-span-full">form_field(config: FormFieldConfig::new("api-key", "API Key"),
                                <input id="api-key" name="api_key" type="password" autocomplete="new-password" :value=$(api_key.get()) @input=$(|event: Event| { api_key.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) :placeholder=$(if id.get().is_empty() { "输入 Provider API Key" } else { "留空以保留现有 API Key" }) :required=$(id.get().is_empty()) aria-describedby="api-key-help">
                                <p class=(FIELD_HINT) id="api-key-help">$(if id.get().is_empty() { "必填，凭据加密保存且不会回显。" } else { "留空保留现有凭据，输入新值即可更换。" })</p>
                            )</div>
                        </div>
                    </section>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-6 [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"模型探测"</h3>
                        <input type="hidden" name="models_probe_status" :value=$(models_probe_status.get())>
                        <div class="grid grid-cols-[minmax(0,1fr)_210px_auto_28px] items-end gap-3 max-[640px]:grid-cols-[minmax(0,1fr)_auto_28px] [&_input]:w-full [&_select]:w-full">
                            form_field(config: FormFieldConfig::new("models-path", "模型列表路径").required(), <input id="models-path" name="models_path" :value=$(models_path.get()) @input=$(|event: Event| { models_path.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) placeholder="/models" required="">)
                            <div class="max-[640px]:col-span-full max-[640px]:row-start-2">form_field(config: FormFieldConfig::new("models-protocol", "探测协议").required(), <select id="models-protocol" name="models_protocol" :value=$(models_protocol.get()) @change=$(|event: Event| { models_protocol.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) required=""><option value="" disabled="">"请选择协议"</option><option value="openai_chat" :disabled=$(!openai_chat.get())>"OpenAIChat"</option><option value="openai_responses" :disabled=$(!openai_responses.get())>"OpenAIResponses"</option><option value="anthropic_messages" :disabled=$(!anthropic_messages.get())>"AnthropicMessages"</option><option value="gemini" :disabled=$(!gemini.get())>"Gemini"</option></select>)</div>
                            <button class=(BUTTON) type="button" :disabled=$(busy.get()) @click=$(async |_event: Event| {
                                if busy.get() { return; }
                                busy.set(true);
                                models_probe_message.set("".to_owned());
                                let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById('provider-editor-form'))).toString())", String::new());
                                let result = raw!("await Promise.resolve(${preview_models}.call(${_payload})).catch(() => ${probe_unavailable})", probe_unavailable.clone());
                                busy.set(false);
                                if result.is_ok() { models_probe_status.set("success".to_owned()); models_probe_message.set(result.unwrap()); }
                                else { models_probe_status.set("failure".to_owned()); models_probe_message.set(result.unwrap_err()); }
                            })>"探测"</button>
                            <span class="group flex h-9 items-center justify-center" role="status" :data-status=$(models_probe_status.get()) :aria-label=$(if models_probe_status.get() == "success" { "探测成功" } else if models_probe_status.get() == "failure" { "探测失败" } else { "尚未探测" })><span class="hidden group-data-[status=unprobed]:inline-flex">icon(data: CHECK_CIRCLE_FILLED, attrs: attributes! { class="size-[18px] text-muted" aria-hidden="true" })</span><span class="hidden group-data-[status=success]:inline-flex">icon(data: CHECK_CIRCLE_FILLED, attrs: attributes! { class="size-[18px] text-[#52c41a]" aria-hidden="true" })</span><span class="hidden group-data-[status=failure]:inline-flex">icon(data: CLOSE_CIRCLE_FILLED, attrs: attributes! { class="size-[18px] text-[#ff4d4f]" aria-hidden="true" })</span></span>
                        </div>
                        <p class=(FIELD_HINT)>"使用当前表单配置预览探测；保存 Provider 后记录结果。鉴权方式由探测协议决定。"</p>
                        <p class="mt-2 mb-0 text-[13px] leading-relaxed text-secondary" role="status" :hidden=$(models_probe_message.get().is_empty())>$(models_probe_message.get())</p>
                    </section>
                    <details class="group mt-6 rounded-md border border-border" :open=$(advanced.get())>
                        <summary class="flex cursor-pointer list-none items-center gap-3 px-4 py-3 text-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#91caff] [&::-webkit-details-marker]:hidden max-[640px]:flex-wrap max-[640px]:gap-1.5" @click=$(|event: Event| { event.prevent_default(); advanced.toggle(); })><span>"超时设置"</span><span class="ml-auto text-[13px] text-secondary max-[640px]:order-2 max-[640px]:w-full">"连接 "$(connect_timeout.get())" / 读取 "$(read_timeout.get())" / 写入 "$(write_timeout.get())" ms"</span>icon(data: DOWN_OUTLINED, attrs: attributes! { class="size-3.5 shrink-0 text-muted transition-transform duration-150 group-open:rotate-180 max-[640px]:ml-auto" aria-hidden="true" })</summary>
                        <div class="px-4 pb-4 [&>p]:mt-0 [&>p]:mb-4"><p class=(FIELD_HINT)>"单位为毫秒。读取超时表示等待上游数据的最长间隔。"</p><div class=(class!(FIELDS_GRID, "grid-cols-3! gap-4! max-[640px]:grid-cols-1!"))>
                            form_field(config: FormFieldConfig::new("connect-timeout", "连接超时").required(), <input id="connect-timeout" name="connect_timeout_ms" type="number" min="1" :value=$(connect_timeout.get()) @input=$(|event: Event| connect_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                            form_field(config: FormFieldConfig::new("read-timeout", "读取超时").required(), <input id="read-timeout" name="read_timeout_ms" type="number" min="1" :value=$(read_timeout.get()) @input=$(|event: Event| read_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                            form_field(config: FormFieldConfig::new("write-timeout", "写入超时").required(), <input id="write-timeout" name="write_timeout_ms" type="number" min="1" :value=$(write_timeout.get()) @input=$(|event: Event| write_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                        </div></div>
                    </details>
                </div>
                <footer class="flex shrink-0 justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4 max-[640px]:px-4"><button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button><button class=(class!(BUTTON, PRIMARY_BUTTON)) type="submit" :disabled=$(busy.get())>$(if id.get().is_empty() { "创建 Provider" } else { "保存修改" })</button></footer>
            </form>
        )
    })
}
