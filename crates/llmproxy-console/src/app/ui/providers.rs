// Topcoat's procedure and shard handlers receive native form and signal fields separately.
#![expect(clippy::too_many_arguments)]

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{
    MessagesAuth, ProbeStatus, ProviderInput, ProviderPaths, ProviderView, StoreError,
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{
        content::{Form, Json},
        page, route,
    },
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, class, component, view},
};
use topcoat_ant_design::icons::{
    APPSTORE_OUTLINED, CHECK_CIRCLE_FILLED, CLOSE_CIRCLE_FILLED, DOWN_OUTLINED, INFO_CIRCLE_FILLED,
    PLUS_OUTLINED, SEARCH_OUTLINED,
};
use topcoat_ant_design::{
    FormFieldConfig, NativeDialogConfig, NotificationTone, TagTone, UiLanguage, data_table,
    form_field, native_dialog, native_dialog_close_attributes, notification, popconfirm,
    popconfirm_trigger_attributes, select, tag,
};
use url::{Host, Url};

use crate::app::{
    AppState, check_csrf,
    model_catalog::{DEFAULT_ANTHROPIC_VERSION, query_models},
};

mod actions;
mod editor;

use actions::{action_input, save_input};
pub(crate) use actions::{preview_models, provider_action, save_provider};
use editor::{EditorSignals, editor_trigger, provider_editor};

// Complete class names let Tailwind discover styles in Rust at build time.
pub(super) const PAGE_HEADING: &str = "mb-6 flex min-h-20 items-center justify-between gap-6 max-[640px]:min-h-0 max-[640px]:flex-col max-[640px]:items-start max-[640px]:gap-4 [&_h1]:m-0 [&_h1]:text-[28px] [&_h1]:font-semibold [&_h1]:leading-[1.35] max-[640px]:[&_h1]:text-2xl [&_p]:mt-2 [&_p]:mb-0 [&_p]:text-sm [&_p]:leading-relaxed [&_p]:text-secondary";
pub(super) const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-control-border bg-white px-4 text-sm font-medium leading-5 text-heading hover:border-primary-hover hover:text-primary";
const PRIMARY_BUTTON: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const TEXT_LINK: &str = "border-0 bg-transparent p-0 text-sm leading-[22px] whitespace-nowrap text-primary hover:text-primary-hover";
const FIELDS_GRID: &str = "grid grid-cols-2 items-start gap-5 max-[640px]:grid-cols-1 [&>div]:content-start [&_input:not([type=checkbox])]:w-full [&_select]:w-full [&_label]:text-sm [&_.text-xs]:text-[13px] [&_.text-xs]:leading-relaxed [&_.text-xs]:text-secondary";
const FIELD_HINT: &str = "mt-2 mb-0 text-[13px] leading-relaxed text-secondary";
const CHECKBOX: &str = "group inline-flex cursor-pointer items-center gap-2 whitespace-nowrap border-0 bg-transparent p-0 text-sm text-heading";
const CHECKBOX_MARK: &str = "grid size-4 shrink-0 place-items-center rounded-[3px] border border-control-border bg-white text-[11px] leading-none text-white group-aria-[checked=true]:border-primary group-aria-[checked=true]:bg-primary";

pub(super) const PROTOCOLS: [Protocol; 3] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
];

pub(super) fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "OpenAI Chat",
        Protocol::OpenAiResponses => "OpenAI Responses",
        Protocol::AnthropicMessages => "Anthropic Messages",
    }
}

fn protocol_compact_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Resp",
        Protocol::AnthropicMessages => "Msg",
    }
}

fn parse_protocol(value: &str) -> std::result::Result<Protocol, String> {
    PROTOCOLS
        .into_iter()
        .find(|protocol| protocol.as_str() == value)
        .ok_or_else(|| "请选择有效的协议".to_owned())
}

fn provider_url(provider: &ProviderView) -> String {
    let scheme = if provider.tls { "https" } else { "http" };
    let default_port = if provider.tls { 443 } else { 80 };
    if provider.port == default_port {
        format!("{scheme}://{}", provider.host)
    } else {
        format!("{scheme}://{}:{}", provider.host, provider.port)
    }
}

fn parse_upstream_url(value: &str) -> std::result::Result<(String, u16, bool), String> {
    let value = value.trim();
    let invalid = || "请输入有效的上游地址，例如 https://api.deepseek.com".to_owned();
    if !value.contains("://")
        || value.contains('\\')
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid());
    }
    let url = Url::parse(value).map_err(|_| invalid())?;
    let tls = match url.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err("上游地址须以 http:// 或 https:// 开头".to_owned()),
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("上游地址不能包含用户名、密码、查询参数或 # 片段".to_owned());
    }
    if url.path() != "/" {
        return Err(
            "上游地址只需填写服务地址，例如 https://api.deepseek.com，无需添加 /v1 或接口路径"
                .to_owned(),
        );
    }
    let host = match url.host() {
        Some(Host::Domain(host)) => host.to_owned(),
        Some(Host::Ipv4(host)) => host.to_string(),
        Some(Host::Ipv6(_)) => return Err("上游地址目前支持域名或 IPv4 地址".to_owned()),
        None => return Err(invalid()),
    };
    let port = url.port_or_known_default().ok_or_else(invalid)?;
    if port == 0 {
        return Err("上游地址中的端口须为 1–65535".to_owned());
    }
    Ok((host, port, tls))
}

#[derive(Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    state: String,
}

#[page]
pub async fn list(cx: &Cx, Form(query): Form<ListQuery>) -> Result<impl View> {
    let _ = cx;
    Ok(view! { provider_workspace(query: &query, edit_id: "", editor_open: false) })
}

#[component]
async fn provider_workspace(
    cx: &Cx,
    query: &ListQuery,
    edit_id: &str,
    editor_open: bool,
) -> Result<impl View> {
    let success = signal(cx, String::new);
    let failure = signal(cx, String::new);
    let q = query.q.clone();
    let protocol = query.protocol.clone();
    let state = query.state.clone();
    let edit_id = edit_id.to_owned();
    Ok(view! {
        notification(message: &success, title: "操作成功", tone: NotificationTone::Success, language: UiLanguage::ChineseSimplified)
        notification(message: &failure, title: "操作失败", tone: NotificationTone::Error, language: UiLanguage::ChineseSimplified)
        provider_list(q: $(q), protocol: $(protocol), state: $(state), edit_id: $(edit_id), editor_open: $(editor_open), success: $(success), failure: $(failure))
    })
}

#[shard("/ui/_topcoat/runtime/shards/provider-list")]
pub async fn provider_list(
    cx: &Cx,
    q: String,
    protocol: String,
    state: String,
    edit_id: String,
    editor_open: bool,
    success: Signal<String>,
    failure: Signal<String>,
) -> Result<impl View> {
    let controls = ListSignals {
        refresh: signal(cx, || 0.0),
        busy: signal(cx, || false),
        success,
        failure,
    };
    let _revision = controls.refresh.get();
    let draft_q = signal(cx, || q.clone());
    let draft_protocol = signal(cx, || protocol.clone());
    let draft_state = signal(cx, || state.clone());
    let applied_q = signal(cx, || q);
    let applied_protocol = signal(cx, || protocol);
    let applied_state = signal(cx, || state);
    let all = app_context::<AppState>(cx).store.list().await?;
    let query = ListQuery {
        q: applied_q.get(),
        protocol: applied_protocol.get(),
        state: applied_state.get(),
    };
    let search = query.q.trim().to_lowercase();
    let filtered: Vec<_> = all
        .iter()
        .filter(|provider| {
            (search.is_empty()
                || provider.name.to_lowercase().contains(&search)
                || provider.host.to_lowercase().contains(&search))
                && (query.protocol.is_empty()
                    || provider
                        .paths
                        .supported()
                        .iter()
                        .any(|protocol| protocol.as_str() == query.protocol))
                && match query.state.as_str() {
                    "enabled" => provider.enabled,
                    "disabled" => !provider.enabled,
                    _ => true,
                }
        })
        .cloned()
        .collect();
    let providers = filtered;
    let defaults = if edit_id.is_empty() {
        ProviderForm::default()
    } else {
        app_context::<AppState>(cx)
            .store
            .get(edit_id.parse::<i64>()?)
            .await?
            .into()
    };
    let editor = EditorSignals::new(cx, &defaults, None, editor_open);
    let create = editor_trigger(cx, &editor, ProviderForm::default());
    let csrf = &app_context::<AppState>(cx).csrf;
    let total = all.len();
    let enabled = all.iter().filter(|provider| provider.enabled).count();
    let refresh = controls.refresh.clone();
    let reset = attributes! { cx => @click=$(|_event: Event| {
        draft_q.set("".to_owned());
        draft_protocol.set("".to_owned());
        draft_state.set("".to_owned());
        applied_q.set("".to_owned());
        applied_protocol.set("".to_owned());
        applied_state.set("".to_owned());
        refresh.increment();
    }) };
    Ok(view! {
        provider_editor(editor: &editor, controls: &controls)
        <section class=(PAGE_HEADING)>
            <div><h1>"Providers"</h1><p>"管理上游连接、协议路径与凭据。"</p></div>
            <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())>icon(data: PLUS_OUTLINED, attrs: attributes! { class="size-4 shrink-0" aria-hidden="true" })"新建 Provider"</button>
        </section>
        <section class="providers-panel overflow-visible rounded-lg border border-border bg-white shadow-xs" aria-labelledby="providers-heading">
            <div class="flex items-center justify-between gap-4 px-6 pt-5 max-[640px]:px-4 [&_h2]:m-0 [&_h2]:flex [&_h2]:items-center [&_h2]:gap-2 [&_h2]:text-base [&_h2]:font-semibold"><h2 id="providers-heading">"Provider 列表"<span class="rounded bg-surface px-2 text-[13px] font-normal leading-6 text-secondary">(total)</span></h2><span class="text-[13px] text-secondary">(enabled)" 个已启用"</span></div>
            <form class="flex flex-wrap items-center gap-3 px-6 py-5 max-[640px]:gap-2 max-[640px]:px-4 [&_select]:h-9 [&_select]:min-w-[144px] [&_select]:text-sm max-[640px]:[&_select]:min-w-0 max-[640px]:[&_select]:flex-1" method="get" action="/ui/providers" role="search" @submit=$(|event: Event| {
                event.prevent_default();
                applied_q.set(draft_q.get());
                applied_protocol.set(draft_protocol.get());
                applied_state.set(draft_state.get());
                refresh.increment();
            })>
                <div class="flex h-9 w-[300px] items-center gap-2 rounded-md border border-control-border pl-3 focus-within:border-primary-hover focus-within:ring-2 focus-within:ring-primary/10 max-[640px]:w-full [&_input]:h-8 [&_input]:w-full [&_input]:border-0 [&_input]:bg-transparent [&_input]:pl-0 [&_input]:text-sm [&_input]:shadow-none">icon(data: SEARCH_OUTLINED, attrs: attributes! { class="size-4 shrink-0 text-muted" aria-hidden="true" })<input aria-label="搜索名称或主机" name="q" :value=$(draft_q.get()) @input=$(|event: Event| draft_q.set(event.target.value)) placeholder="搜索名称或主机地址"></div>
                <select name="protocol" aria-label="筛选协议" :value=$(draft_protocol.get()) @change=$(|event: Event| draft_protocol.set(event.target.value))><option value="">"全部协议"</option>for protocol in PROTOCOLS { <option value=(protocol.as_str()) selected=(draft_protocol.get_untracked() == protocol.as_str())>(protocol_label(protocol))</option> }</select>
                <select name="state" aria-label="筛选状态" :value=$(draft_state.get()) @change=$(|event: Event| draft_state.set(event.target.value))><option value="">"全部状态"</option><option value="enabled" selected=(draft_state.get_untracked() == "enabled")>"已启用"</option><option value="disabled" selected=(draft_state.get_untracked() == "disabled")>"已停用"</option></select>
                <button class=(BUTTON) type="submit">"查询"</button>
                if !query.q.is_empty() || !query.protocol.is_empty() || !query.state.is_empty() { <button class=(class!(TEXT_LINK, "px-1")) type="button" (reset.clone())>"重置"</button> }
            </form>
            if providers.is_empty() {
                <div class="border-t border-border px-6 py-12 text-center [&_h3]:mt-4 [&_h3]:mb-2 [&_h3]:text-base [&_h3]:font-medium [&_h3]:text-heading [&_p]:mt-0 [&_p]:mb-6 [&_p]:text-sm [&_p]:text-secondary">icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="mx-auto block size-10 text-[#bfbfbf]" aria-hidden="true" })<h3>(if all.is_empty() { "连接第一个模型服务" } else { "没有找到匹配的 Provider" })</h3><p>(if all.is_empty() { "添加上游地址与 API Key，即可开始管理你的模型连接。" } else { "尝试调整搜索关键词，或清除筛选条件。" })</p>if all.is_empty() { <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())>"新建 Provider"</button> } else { <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (reset.clone())>"清除筛选"</button> }</div>
            } else {
                data_table(label: "Provider 列表", attrs: attributes! { class="min-w-[900px] [&_th]:px-6! [&_th]:text-[13px]! [&_td]:px-6! [&_td]:py-4! [&_td]:text-sm! [&_.gr-tag]:text-[13px]" },
                    <thead><tr><th>"名称 / 协议"</th><th>"上游地址"</th><th>"状态"</th><th>"凭据"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        #[key(provider.id)]
                        for provider in &providers {
                            <tr id=(format!("provider-{}", provider.id))>
                                <td><button class="block border-0 bg-transparent p-0 text-left text-sm font-medium leading-[22px] text-heading hover:text-primary" type="button" (editor_trigger(cx, &editor, provider.clone().into()))>(provider.name.as_str())</button><span class="mt-1 block whitespace-nowrap text-[13px] leading-5 text-secondary" title=(provider.paths.supported().into_iter().map(protocol_label).collect::<Vec<_>>().join(" / "))>(provider.paths.supported().into_iter().map(protocol_compact_label).collect::<Vec<_>>().join(" · "))</span></td>
                                <td><span class="whitespace-nowrap text-sm text-heading">(provider_url(provider))</span><span class="mt-1 block text-[13px] leading-5 text-secondary">"读取超时 "(provider.read_timeout_ms / 1000)" 秒"</span></td>
                                <td><div class="flex max-w-[185px] flex-wrap gap-[5px]">tag(tone: if provider.enabled { TagTone::Success } else { TagTone::Default }, (if provider.enabled { "已启用" } else { "已停用" }))</div></td>
                                <td><span class="inline-flex items-center gap-1.5 whitespace-nowrap text-[13px] text-secondary">if provider.key_configured { icon(data: CHECK_CIRCLE_FILLED, attrs: attributes! { class="size-3.5 text-muted" aria-hidden="true" }) }(if provider.key_configured { "已配置" } else { "未配置" })</span></td>
                                <td><div class="flex min-w-[204px] items-center justify-end gap-3 whitespace-nowrap">
                                    <button class=(TEXT_LINK) type="button" (editor_trigger(cx, &editor, provider.clone().into()))>"编辑"</button>
                                    if provider.enabled {
                                        provider_action_confirmation(controls: &controls, provider: provider, csrf: csrf, delete: false)
                                    } else {
                                        action_form(controls: &controls, csrf: csrf, provider: provider, action: "enable", label: "启用".to_owned())
                                        provider_action_confirmation(controls: &controls, provider: provider, csrf: csrf, delete: true)
                                    }
                                </div></td>
                            </tr>
                        }
                    </tbody>
                )
                <div class="border-t border-border px-6 py-4 text-[13px] text-secondary max-[640px]:px-4">"显示 "(providers.len())" / "(total)" 个 Provider"</div>
            }
        </section>
        <p class="mt-4 mb-0 flex items-start gap-2 text-[13px] leading-relaxed text-secondary">icon(data: INFO_CIRCLE_FILLED, attrs: attributes! { class="mt-1 size-3.5 shrink-0 text-muted" aria-hidden="true" })"请先在 Models 中添加上游模型，再到 Model Routes 配置对外模型名与候选顺序。"</p>
    })
}

#[component]
async fn provider_action_confirmation(
    cx: &Cx,
    provider: &ProviderView,
    csrf: &str,
    controls: &ListSignals,
    delete: bool,
) -> Result<impl View> {
    let (action, label) = if delete {
        ("delete", "删除")
    } else {
        ("disable", "停用")
    };
    let id = format!("{action}-{}", provider.id);
    let title = format!("确认{label}「{}」？", provider.name);
    let trigger = popconfirm_trigger_attributes(cx, &id);
    let submit = action_submit(cx, controls, csrf, provider, action);
    let busy = &controls.busy;
    Ok(view! {
        <button class=(class!(TEXT_LINK, "text-[#cf1322]! hover:text-[#ff4d4f]!")) type="button" (trigger) :disabled=$(busy.get())>(label)</button>
        popconfirm(id: id.as_str(), title: title.as_str(), language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="[&_footer]:items-center [&_footer_.gr-button]:h-8! [&_footer_.gr-button]:w-[88px]! [&_footer_.gr-button]:px-3! [&_footer_.gr-button]:py-1! [&_footer_.gr-button]:text-sm! [&_footer_.gr-button]:leading-[22px]!" },
            <form class="m-0 inline-flex" action="/ui/providers/action" method="post" (submit)><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><button class="gr-button gr-button-danger" type="submit" :disabled=$(busy.get())>(format!("确认{label}"))</button></form>
        )
    })
}

#[component]
async fn action_form(
    cx: &Cx,
    controls: &ListSignals,
    csrf: &str,
    provider: &ProviderView,
    action: &str,
    label: String,
) -> Result<impl View> {
    let submit = action_submit(cx, controls, csrf, provider, action);
    let busy = &controls.busy;
    Ok(
        view! { <form class="m-0 inline-flex" action="/ui/providers/action" method="post" (submit)><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><button class=(TEXT_LINK) type="submit" :disabled=$(busy.get())>(label)</button></form> },
    )
}

#[derive(Default, Deserialize)]
pub struct EditQuery {
    id: Option<i64>,
}

#[derive(Deserialize)]
pub struct ProviderForm {
    #[serde(default)]
    csrf: String,
    id: Option<String>,
    version: Option<String>,
    name: String,
    #[serde(default)]
    openai_chat: bool,
    #[serde(default)]
    openai_chat_path: String,
    #[serde(default)]
    openai_responses: bool,
    #[serde(default)]
    openai_responses_path: String,
    #[serde(default)]
    anthropic_messages: bool,
    #[serde(default)]
    anthropic_messages_path: String,
    upstream_url: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    api_key: String,
    models_path: String,
    models_protocol: String,
    #[serde(default = "default_probe_status")]
    models_probe_status: String,
    #[serde(default)]
    anthropic_version: String,
    #[serde(default = "default_messages_auth")]
    messages_auth: String,
    connect_timeout_ms: String,
    read_timeout_ms: String,
    write_timeout_ms: String,
}

fn default_probe_status() -> String {
    ProbeStatus::Unprobed.as_str().to_owned()
}

fn default_messages_auth() -> String {
    MessagesAuth::ApiKey.as_str().to_owned()
}

impl Default for ProviderForm {
    fn default() -> Self {
        Self {
            csrf: String::new(),
            id: None,
            version: None,
            name: String::new(),
            openai_chat: true,
            openai_chat_path: Protocol::OpenAiChat.upstream_path().to_owned(),
            openai_responses: false,
            openai_responses_path: Protocol::OpenAiResponses.upstream_path().to_owned(),
            anthropic_messages: false,
            anthropic_messages_path: Protocol::AnthropicMessages.upstream_path().to_owned(),
            upstream_url: String::new(),
            enabled: true,
            api_key: String::new(),
            models_path: "/models".to_owned(),
            models_protocol: Protocol::OpenAiChat.as_str().to_owned(),
            models_probe_status: ProbeStatus::Unprobed.as_str().to_owned(),
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_owned(),
            messages_auth: default_messages_auth(),
            connect_timeout_ms: "10000".to_owned(),
            read_timeout_ms: "60000".to_owned(),
            write_timeout_ms: "30000".to_owned(),
        }
    }
}

impl From<ProviderView> for ProviderForm {
    fn from(provider: ProviderView) -> Self {
        let supports_anthropic = provider.paths.anthropic_messages.is_some();
        Self {
            upstream_url: provider_url(&provider),
            csrf: String::new(),
            id: Some(provider.id.to_string()),
            version: Some(provider.version.to_string()),
            name: provider.name,
            openai_chat: provider.paths.openai_chat.is_some(),
            openai_chat_path: provider
                .paths
                .openai_chat
                .unwrap_or_else(|| Protocol::OpenAiChat.upstream_path().to_owned()),
            openai_responses: provider.paths.openai_responses.is_some(),
            openai_responses_path: provider
                .paths
                .openai_responses
                .unwrap_or_else(|| Protocol::OpenAiResponses.upstream_path().to_owned()),
            anthropic_messages: provider.paths.anthropic_messages.is_some(),
            anthropic_messages_path: provider
                .paths
                .anthropic_messages
                .unwrap_or_else(|| Protocol::AnthropicMessages.upstream_path().to_owned()),
            enabled: provider.enabled,
            api_key: String::new(),
            models_path: provider.models_path,
            models_protocol: provider.models_protocol.as_str().to_owned(),
            models_probe_status: provider.models_probe_status.as_str().to_owned(),
            anthropic_version: provider.anthropic_version.unwrap_or_else(|| {
                if supports_anthropic {
                    String::new()
                } else {
                    DEFAULT_ANTHROPIC_VERSION.to_owned()
                }
            }),
            messages_auth: provider.messages_auth.as_str().to_owned(),
            connect_timeout_ms: provider.connect_timeout_ms.to_string(),
            read_timeout_ms: provider.read_timeout_ms.to_string(),
            write_timeout_ms: provider.write_timeout_ms.to_string(),
        }
    }
}

impl ProviderForm {
    fn preview_input(&self) -> std::result::Result<ProviderInput, String> {
        let protocol = parse_protocol(&self.models_protocol)?;
        let selected = match protocol {
            Protocol::OpenAiChat => self.openai_chat,
            Protocol::OpenAiResponses => self.openai_responses,
            Protocol::AnthropicMessages => self.anthropic_messages,
        };
        if !selected {
            return Err("模型探测协议必须是已选择的接口协议".to_owned());
        }
        let (host, port, tls) = parse_upstream_url(&self.upstream_url)?;
        Ok(ProviderInput {
            name: "探测预览".to_owned(),
            paths: ProviderPaths::single(protocol),
            host,
            port,
            tls,
            api_key: self.api_key.clone(),
            enabled: true,
            models_path: self.models_path.clone(),
            models_protocol: protocol,
            anthropic_version: if protocol == Protocol::AnthropicMessages
                && !self.anthropic_version.trim().is_empty()
            {
                Some(self.anthropic_version.trim().to_owned())
            } else {
                None
            },
            messages_auth: MessagesAuth::parse(&self.messages_auth)
                .ok_or_else(|| "Messages 鉴权方式无效".to_owned())?,
            connect_timeout_ms: 10_000,
            read_timeout_ms: 60_000,
            write_timeout_ms: 30_000,
        })
    }

    fn input(&self) -> std::result::Result<ProviderInput, String> {
        let (host, port, tls) = parse_upstream_url(&self.upstream_url)?;
        let timeout = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| "超时必须是以毫秒为单位的正整数".to_owned())
        };
        Ok(ProviderInput {
            name: self.name.clone(),
            paths: ProviderPaths {
                openai_chat: self.openai_chat.then(|| self.openai_chat_path.clone()),
                openai_responses: self
                    .openai_responses
                    .then(|| self.openai_responses_path.clone()),
                anthropic_messages: self
                    .anthropic_messages
                    .then(|| self.anthropic_messages_path.clone()),
            },
            host,
            port,
            tls,
            enabled: self.enabled,
            api_key: self.api_key.clone(),
            models_path: self.models_path.clone(),
            models_protocol: parse_protocol(&self.models_protocol)?,
            anthropic_version: if !self.anthropic_messages
                || self.anthropic_version.trim().is_empty()
            {
                None
            } else {
                Some(self.anthropic_version.trim().to_owned())
            },
            messages_auth: MessagesAuth::parse(&self.messages_auth)
                .ok_or_else(|| "Messages 鉴权方式无效".to_owned())?,
            connect_timeout_ms: timeout(&self.connect_timeout_ms)?,
            read_timeout_ms: timeout(&self.read_timeout_ms)?,
            write_timeout_ms: timeout(&self.write_timeout_ms)?,
        })
    }
}

#[page("./form")]
pub async fn form(cx: &Cx, Form(query): Form<EditQuery>) -> Result<impl View> {
    let _ = cx;
    let filters = ListQuery::default();
    let edit_id = query.id.map(|id| id.to_string()).unwrap_or_default();
    Ok(view! { provider_workspace(query: &filters, edit_id: &edit_id, editor_open: true) })
}

type Outcome = std::result::Result<String, String>;

#[route(POST "./save")]
pub async fn save(cx: &Cx, Form(input): Form<ProviderForm>) -> Result<Json<Outcome>> {
    Ok(Json(save_input(cx, input).await?))
}

#[derive(Deserialize)]
pub struct ActionForm {
    #[serde(default)]
    csrf: String,
    id: i64,
    version: u64,
    action: String,
}

#[route(POST "./action")]
pub async fn perform_action(cx: &Cx, Form(input): Form<ActionForm>) -> Result<Json<Outcome>> {
    Ok(Json(action_input(cx, input).await?))
}

struct ListSignals {
    refresh: Signal<f64>,
    busy: Signal<bool>,
    success: Signal<String>,
    failure: Signal<String>,
}

fn action_submit(
    cx: &Cx,
    controls: &ListSignals,
    csrf: &str,
    provider: &ProviderView,
    action: &str,
) -> Attributes {
    let ListSignals {
        refresh,
        busy,
        success,
        failure,
    } = controls;
    let id = provider.id.to_string();
    let version = provider.version.to_string();
    let unavailable: Outcome = Err(format!(
        "「{}」请求失败或结果未确认，请检查列表状态后重试",
        provider.name
    ));
    attributes! { cx => @submit=$(async |event: Event| {
        event.prevent_default();
        if busy.get() { return; }
        busy.set(true);
        success.set("".to_owned());
        failure.set("".to_owned());
        // Native procedures need a transport rejection handler.
        let result = raw!("await Promise.resolve(${provider_action}.call(${csrf}, ${id}, ${version}, ${action})).catch(() => ${unavailable})", unavailable.clone());
        busy.set(false);
        if result.is_ok() {
            success.set(result.unwrap());
            refresh.increment();
        } else { failure.set(result.unwrap_err()); }
    }) }
}
