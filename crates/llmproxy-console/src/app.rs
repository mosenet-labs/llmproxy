// Topcoat's procedure and shard handlers receive native form and signal fields separately.
#![expect(clippy::too_many_arguments)]

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{
    ModelProbeTarget, ProbeStatus, ProviderInput, ProviderPaths, ProviderStore, ProviderView,
    StoreError,
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{
        Body, Next, Slot,
        content::{Form, Json},
        error::{forbidden, see_other},
        layer, layout, page,
        request::{headers, uri},
        response::Response,
        route,
    },
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, class, component, view},
};
use topcoat_ant_design::icons::{
    APARTMENT_OUTLINED, APPSTORE_OUTLINED, ARROW_RIGHT_OUTLINED, CHECK_CIRCLE_FILLED,
    CLOSE_CIRCLE_FILLED, DOWN_OUTLINED, INFO_CIRCLE_FILLED, PLUS_OUTLINED, SEARCH_OUTLINED,
};
use topcoat_ant_design::{
    DialogConfig, FormFieldConfig, NotificationTone, TagTone, UiLanguage, data_table, dialog,
    dialog_close_attributes, form_field, head_assets, notification, popconfirm,
    popconfirm_trigger_attributes, tag,
};
use url::{Host, Url};

pub struct AppState {
    pub store: ProviderStore,
    pub csrf: String,
    pub port: u16,
    pub telemetry: crate::observability::ConsoleTelemetry,
}

// Complete class names let Tailwind discover styles in Rust at build time.
const PAGE_HEADING: &str = "mb-6 flex min-h-20 items-center justify-between gap-6 max-[640px]:min-h-0 max-[640px]:flex-col max-[640px]:items-start max-[640px]:gap-4 [&_h1]:m-0 [&_h1]:text-[28px] [&_h1]:font-semibold [&_h1]:leading-[1.35] max-[640px]:[&_h1]:text-2xl [&_p]:mt-2 [&_p]:mb-0 [&_p]:text-sm [&_p]:leading-relaxed [&_p]:text-secondary";
const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-control-border bg-white px-4 text-sm font-medium leading-5 text-heading hover:border-primary-hover hover:text-primary";
const PRIMARY_BUTTON: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const TEXT_LINK: &str = "border-0 bg-transparent p-0 text-sm leading-[22px] whitespace-nowrap text-primary hover:text-primary-hover";
const NAV_ITEM: &str = "flex min-h-11 items-center gap-3 whitespace-nowrap rounded-md px-3 py-2.5 text-sm text-secondary hover:bg-surface hover:text-primary aria-[current=page]:bg-primary-soft aria-[current=page]:font-medium aria-[current=page]:text-[#0958d9] max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-[13px]";
const FIELDS_GRID: &str = "grid grid-cols-2 items-start gap-5 max-[640px]:grid-cols-1 [&>div]:content-start [&_input:not([type=checkbox])]:w-full [&_select]:w-full [&_label]:text-sm [&_.text-xs]:text-[13px] [&_.text-xs]:leading-relaxed [&_.text-xs]:text-secondary";
const FIELD_HINT: &str = "mt-2 mb-0 text-[13px] leading-relaxed text-secondary";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const CHECKBOX: &str = "group inline-flex cursor-pointer items-center gap-2 whitespace-nowrap border-0 bg-transparent p-0 text-sm text-heading";
const CHECKBOX_MARK: &str = "grid size-4 shrink-0 place-items-center rounded-[3px] border border-control-border bg-white text-[11px] leading-none text-white group-aria-[checked=true]:border-primary group-aria-[checked=true]:bg-primary";

const PROTOCOLS: [Protocol; 3] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
];

#[route(GET "/ui/providers")]
pub async fn providers_redirect() -> Result<topcoat::router::error::SeeOther> {
    Ok(see_other("/ui"))
}

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "OpenAI Chat",
        Protocol::OpenAiResponses => "OpenAI Responses",
        Protocol::AnthropicMessages => "Anthropic Messages",
    }
}

fn protocol_short_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Responses",
        Protocol::AnthropicMessages => "Messages",
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

#[layer("/")]
pub async fn protect(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let state = app_context::<AppState>(cx);
    let request_headers = headers(cx);
    let authority = request_headers
        .get("host")
        .and_then(|host| host.to_str().ok());
    let localhost = format!("localhost:{}", state.port);
    let loopback = format!("127.0.0.1:{}", state.port);
    if !matches!(authority, Some(host) if host == localhost || host == loopback) {
        return Err(forbidden().into());
    }
    if let Some(origin) = request_headers.get("origin") {
        let allowed = origin.to_str().ok().is_some_and(|origin| {
            origin == format!("http://{localhost}") || origin == format!("http://{loopback}")
        });
        if !allowed {
            return Err(forbidden().into());
        }
    }
    let mut response = next.run(cx, body).await?;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse()?);
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse()?);
    response
        .headers_mut()
        .insert("x-frame-options", "DENY".parse()?);
    response
        .headers_mut()
        .insert("referrer-policy", "same-origin".parse()?);
    Ok(response)
}

fn check_csrf(cx: &Cx, supplied: &str) -> Result<()> {
    let expected = app_context::<AppState>(cx).csrf.as_bytes();
    let supplied = supplied.as_bytes();
    let difference = expected
        .iter()
        .zip(supplied)
        .fold(0, |acc, (a, b)| acc | (a ^ b));
    if supplied.len() != expected.len() || difference != 0 {
        return Err(forbidden().into());
    }
    Ok(())
}

#[layout("/")]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let routes_page = uri(cx).path() == "/ui/routes";
    let page_title = if routes_page {
        "路由概览"
    } else {
        "Provider 管理"
    };
    Ok(view! {
        <!DOCTYPE html>
        <html lang="zh-CN">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <meta name="color-scheme" content="light">
                <title>(format!("{page_title} · LLMProxy"))</title>
                head_assets()
                <link rel="stylesheet" href="/ui/assets/console.css">
                topcoat::runtime::script()
            </head>
            <body>
                <div class="grid min-h-screen grid-cols-[216px_minmax(0,1fr)] max-[900px]:grid-cols-[176px_minmax(0,1fr)] max-[640px]:block">
                    <aside class="sticky top-0 flex h-screen flex-col border-r border-border bg-white px-3 max-[640px]:static max-[640px]:h-auto max-[640px]:border-r-0 max-[640px]:border-b max-[640px]:px-4 max-[640px]:pb-2">
                        <a class="flex h-16 shrink-0 items-center gap-3 px-3 text-lg font-semibold max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-base max-[640px]:h-14 max-[640px]:px-0" href="/ui"><span class="grid size-8 place-items-center rounded-lg bg-primary text-xl font-bold text-white" aria-hidden="true">"L"</span><strong>"LLMProxy"</strong></a>
                        <nav class="mt-4 grid gap-1 max-[640px]:mt-0 max-[640px]:grid-cols-2" aria-label="主导航">
                            <a class=(NAV_ITEM) href="/ui" aria-current=(if routes_page { None } else { Some("page") })>icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"Provider 管理"</a>
                            <a class=(NAV_ITEM) href="/ui/routes" aria-current=(if routes_page { Some("page") } else { None })>icon(data: APARTMENT_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"路由概览"</a>
                        </nav>
                        <div class="mt-auto border-t border-border px-3 py-5 text-[13px] text-muted max-[640px]:hidden">"本地开发环境"</div>
                    </aside>
                    <div class="flex min-w-0 flex-col">
                        <header class="h-16 shrink-0 border-b border-border bg-white text-sm text-secondary max-[640px]:hidden"><div class="mx-auto flex size-full max-w-[1480px] items-center px-7 [&_strong]:font-medium [&_strong]:text-heading"><span>"控制台"<span class="mx-3 text-[#bfbfbf]">"/"</span><strong>(page_title)</strong></span></div></header>
                        <main id="main" class="mx-auto w-full max-w-[1480px] flex-1 p-7 pb-12 max-[640px]:px-4 max-[640px]:pt-6 max-[640px]:pb-8">(slot)</main>
                    </div>
                </div>
            </body>
        </html>
    })
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

#[page("/ui")]
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

#[page("/ui/routes")]
pub async fn routes(cx: &Cx) -> Result<impl View> {
    let all = app_context::<AppState>(cx).store.list().await?;
    Ok(view! {
        <section class=(PAGE_HEADING)>
            <div><h1>"路由概览"</h1><p>"查看各协议入口与当前使用的 Provider。"</p></div>
            <a class=(BUTTON) href="/ui">"管理 Provider"</a>
        </section>
        <section class="grid grid-cols-3 gap-5 max-[1180px]:grid-cols-1" id="routes" aria-label="三个协议的当前 Provider">
            for protocol in PROTOCOLS {
                <a class="group min-w-0 rounded-lg border border-border bg-white p-6 shadow-xs hover:border-[#91caff] max-[640px]:p-5" href=(format!("/ui?protocol={}", protocol.as_str()))>
                    <div class="flex items-center gap-3"><span class="grid size-8 shrink-0 place-items-center rounded-md bg-primary-soft text-sm font-semibold text-primary" aria-hidden="true">(match protocol { Protocol::OpenAiChat => "C", Protocol::OpenAiResponses => "R", Protocol::AnthropicMessages => "A" })</span><span class="text-sm font-medium text-heading">(protocol_label(protocol))</span></div>
                    if let Some(provider) = all.iter().find(|provider| provider.active_protocols.contains(&protocol)) {
                        <strong class="mt-6 block truncate text-xl font-semibold leading-7">(provider.name.as_str())</strong><span class="mt-2 flex items-center gap-2 text-[13px] text-secondary">icon(data: CHECK_CIRCLE_FILLED, attrs: attributes! { class="size-3.5 text-[#389e0d]" aria-hidden="true" })"当前 Provider"</span>
                    } else {
                        <strong class="mt-6 block text-xl font-medium leading-7 text-secondary">"尚未分配"</strong><span class="mt-2 block text-[13px] text-secondary">"启用 Provider 后设为当前服务"</span>
                    }
                    <div class="mt-6 flex items-center justify-between gap-3 border-t border-border pt-4"><code class="truncate font-mono text-[13px] text-secondary">(protocol.upstream_path())</code>icon(data: ARROW_RIGHT_OUTLINED, attrs: attributes! { class="size-4 shrink-0 text-muted group-hover:text-primary" aria-hidden="true" })</div>
                </a>
            }
        </section>
    })
}

#[shard]
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
    let all = app_context::<AppState>(cx).store.list().await?;
    let query = ListQuery { q, protocol, state };
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
                    "active" => provider.active,
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
    Ok(view! {
        provider_editor(editor: &editor, controls: &controls)
        <section class=(PAGE_HEADING)>
            <div><h1>"Provider 管理"</h1><p>"管理上游连接与凭据，为每种协议选择当前服务。"</p></div>
            <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())>icon(data: PLUS_OUTLINED, attrs: attributes! { class="size-4 shrink-0" aria-hidden="true" })"新建 Provider"</button>
        </section>
        <section class="providers-panel overflow-visible rounded-lg border border-border bg-white shadow-xs" aria-labelledby="providers-heading">
            <div class="flex items-center justify-between gap-4 px-6 pt-5 max-[640px]:px-4 [&_h2]:m-0 [&_h2]:flex [&_h2]:items-center [&_h2]:gap-2 [&_h2]:text-base [&_h2]:font-semibold"><h2 id="providers-heading">"Provider 列表"<span class="rounded bg-surface px-2 text-[13px] font-normal leading-6 text-secondary">(total)</span></h2><span class="text-[13px] text-secondary">(enabled)" 个已启用"</span></div>
            <form class="flex flex-wrap items-center gap-3 px-6 py-5 max-[640px]:gap-2 max-[640px]:px-4 [&_select]:h-9 [&_select]:min-w-[144px] [&_select]:text-sm max-[640px]:[&_select]:min-w-0 max-[640px]:[&_select]:flex-1" method="get" action="/ui" role="search">
                <div class="flex h-9 w-[300px] items-center gap-2 rounded-md border border-control-border pl-3 focus-within:border-primary-hover focus-within:ring-2 focus-within:ring-primary/10 max-[640px]:w-full [&_input]:h-8 [&_input]:w-full [&_input]:border-0 [&_input]:bg-transparent [&_input]:pl-0 [&_input]:text-sm [&_input]:shadow-none">icon(data: SEARCH_OUTLINED, attrs: attributes! { class="size-4 shrink-0 text-muted" aria-hidden="true" })<input aria-label="搜索名称或主机" name="q" value=(query.q.as_str()) placeholder="搜索名称或主机地址"></div>
                <select name="protocol" aria-label="筛选协议"><option value="">"全部协议"</option>for protocol in PROTOCOLS { <option value=(protocol.as_str()) selected=(query.protocol == protocol.as_str())>(protocol_label(protocol))</option> }</select>
                <select name="state" aria-label="筛选状态"><option value="">"全部状态"</option><option value="enabled" selected=(query.state == "enabled")>"已启用"</option><option value="disabled" selected=(query.state == "disabled")>"已停用"</option><option value="active" selected=(query.state == "active")>"当前使用"</option></select>
                <button class=(BUTTON) type="submit">"查询"</button>
                if !query.q.is_empty() || !query.protocol.is_empty() || !query.state.is_empty() { <a class=(class!(TEXT_LINK, "px-1")) href="/ui">"重置"</a> }
            </form>
            if providers.is_empty() {
                <div class="border-t border-border px-6 py-12 text-center [&_h3]:mt-4 [&_h3]:mb-2 [&_h3]:text-base [&_h3]:font-medium [&_h3]:text-heading [&_p]:mt-0 [&_p]:mb-6 [&_p]:text-sm [&_p]:text-secondary">icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="mx-auto block size-10 text-[#bfbfbf]" aria-hidden="true" })<h3>(if all.is_empty() { "连接第一个模型服务" } else { "没有找到匹配的 Provider" })</h3><p>(if all.is_empty() { "添加上游地址与 API Key，即可开始管理你的模型连接。" } else { "尝试调整搜索关键词，或清除筛选条件。" })</p>if all.is_empty() { <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())>"新建 Provider"</button> } else { <a class=(class!(BUTTON, PRIMARY_BUTTON)) href="/ui">"清除筛选"</a> }</div>
            } else {
                data_table(label: "Provider 列表", attrs: attributes! { class="min-w-[900px] [&_th]:px-6! [&_th]:text-[13px]! [&_td]:px-6! [&_td]:py-4! [&_td]:text-sm! [&_.gr-tag]:text-[13px]" },
                    <thead><tr><th>"名称 / 协议"</th><th>"上游地址"</th><th>"状态"</th><th>"凭据"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        for provider in &providers {
                            <tr id=(format!("provider-{}", provider.id))>
                                <td><button class="block border-0 bg-transparent p-0 text-left text-sm font-medium leading-[22px] text-heading hover:text-primary" type="button" (editor_trigger(cx, &editor, provider.clone().into()))>(provider.name.as_str())</button><span class="mt-1 block text-[13px] leading-5 text-secondary">(provider.paths.supported().into_iter().map(protocol_short_label).collect::<Vec<_>>().join(" / "))</span></td>
                                <td><span class="whitespace-nowrap text-sm text-heading">(provider_url(provider))</span><span class="mt-1 block text-[13px] leading-5 text-secondary">"读取超时 "(provider.read_timeout_ms / 1000)" 秒"</span></td>
                                <td><div class="flex max-w-[185px] flex-wrap gap-[5px]">tag(tone: if provider.enabled { TagTone::Success } else { TagTone::Default }, (if provider.enabled { "已启用" } else { "已停用" })) for active in &provider.active_protocols { tag(tone: TagTone::Processing, (format!("当前 {}", protocol_label(*active)))) }</div></td>
                                <td><span class="inline-flex items-center gap-1.5 whitespace-nowrap text-[13px] text-secondary">if provider.key_configured { icon(data: CHECK_CIRCLE_FILLED, attrs: attributes! { class="size-3.5 text-muted" aria-hidden="true" }) }(if provider.key_configured { "已配置" } else { "未配置" })</span></td>
                                <td><div class="flex min-w-[125px] flex-wrap items-center justify-end gap-3">
                                    <button class=(TEXT_LINK) type="button" (editor_trigger(cx, &editor, provider.clone().into()))>"编辑"</button>
                                    if provider.enabled { for protocol in provider.paths.supported() { if !provider.active_protocols.contains(&protocol) {
                                        action_form(controls: &controls, csrf: csrf, provider: provider, action: "activate", protocol: protocol.as_str(), label: format!("设为当前 {}", protocol_label(protocol)))
                                    } } }
                                    if provider.enabled {
                                        provider_action_confirmation(controls: &controls, provider: provider, csrf: csrf, delete: false)
                                    } else {
                                        action_form(controls: &controls, csrf: csrf, provider: provider, action: "enable", protocol: "", label: "启用".to_owned())
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
        <p class="mt-4 mb-0 flex items-start gap-2 text-[13px] leading-relaxed text-secondary">icon(data: INFO_CIRCLE_FILLED, attrs: attributes! { class="mt-1 size-3.5 shrink-0 text-muted" aria-hidden="true" })"设为当前服务后，该协议的新请求会使用此 Provider。"</p>
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
    let submit = action_submit(cx, controls, csrf, provider, action, "");
    let busy = &controls.busy;
    Ok(view! {
        <button class=(class!(TEXT_LINK, "text-[#cf1322]! hover:text-[#ff4d4f]!" if delete)) type="button" (trigger) :disabled=$(busy.get())>(label)</button>
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
    protocol: &str,
    label: String,
) -> Result<impl View> {
    let submit = action_submit(cx, controls, csrf, provider, action, protocol);
    let busy = &controls.busy;
    Ok(
        view! { <form class="m-0 inline-flex" action="/ui/providers/action" method="post" (submit)><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><input type="hidden" name="protocol" value=(protocol)><button class=(TEXT_LINK) type="submit" :disabled=$(busy.get())>(label)</button></form> },
    )
}

#[procedure]
pub async fn preview_models(cx: &Cx, payload: String) -> Result<Outcome> {
    let input: ProviderForm = serde_json::from_str(&payload)
        .map_err(|_| topcoat::router::error::bad_request("无效的 Provider 表单"))?;
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let name = if input.name.trim().is_empty() {
        "当前配置".to_owned()
    } else {
        input.name.clone()
    };
    let id = input
        .id
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok());
    let result = match input.preview_input() {
        Ok(candidate) => match state.store.preview_target(id, candidate).await {
            Ok(target) => query_models(target).await,
            Err(error) => Err(error.to_string()),
        },
        Err(error) => Err(error),
    };
    state.telemetry.provider_operation(
        "preview_models",
        id,
        Some(&name),
        result.as_ref().err().map(|_| &StoreError::Internal),
    );
    Ok(probe_message(&name, result))
}

fn probe_message(name: &str, result: std::result::Result<Vec<String>, String>) -> Outcome {
    result
        .map(|models| {
            if models.is_empty() {
                format!("「{name}」模型探测成功，上游未返回模型 ID")
            } else {
                format!(
                    "「{name}」探测到 {} 个模型：{}",
                    models.len(),
                    models.join("、")
                )
            }
        })
        .map_err(|error| format!("「{name}」模型探测失败：{error}"))
}

async fn query_models(target: ModelProbeTarget) -> std::result::Result<Vec<String>, String> {
    let scheme = if target.tls { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}{}", target.host, target.port, target.path);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| "无法创建模型探测客户端".to_owned())?;
    let request = client.get(url).header("accept", "application/json");
    let request = match target.protocol {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => request.bearer_auth(&target.secret),
        Protocol::AnthropicMessages => request.header("x-api-key", &target.secret).header(
            "anthropic-version",
            target
                .anthropic_version
                .as_deref()
                .unwrap_or(DEFAULT_ANTHROPIC_VERSION),
        ),
    };
    let mut response = request
        .send()
        .await
        .map_err(|_| "无法连接上游模型列表接口".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("上游返回 HTTP {}", response.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "读取上游响应失败".to_owned())?
    {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("模型列表响应超过 1 MiB".to_owned());
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "上游没有返回有效 JSON".to_owned())?;
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "上游响应缺少 data 模型列表".to_owned())?;
    Ok(data
        .iter()
        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
        .filter(|id| !id.is_empty() && id.len() <= 200)
        .take(100)
        .map(str::to_owned)
        .collect())
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
    connect_timeout_ms: String,
    read_timeout_ms: String,
    write_timeout_ms: String,
}

fn default_probe_status() -> String {
    ProbeStatus::Unprobed.as_str().to_owned()
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
            connect_timeout_ms: timeout(&self.connect_timeout_ms)?,
            read_timeout_ms: timeout(&self.read_timeout_ms)?,
            write_timeout_ms: timeout(&self.write_timeout_ms)?,
        })
    }
}

#[page("/ui/providers/form")]
pub async fn form(cx: &Cx, Form(query): Form<EditQuery>) -> Result<impl View> {
    let _ = cx;
    let filters = ListQuery::default();
    let edit_id = query.id.map(|id| id.to_string()).unwrap_or_default();
    Ok(view! { provider_workspace(query: &filters, edit_id: &edit_id, editor_open: true) })
}

type Outcome = std::result::Result<String, String>;

#[procedure]
pub async fn save_provider(cx: &Cx, payload: String) -> Result<Outcome> {
    let input: ProviderForm = serde_json::from_str(&payload)
        .map_err(|_| topcoat::router::error::bad_request("无效的 Provider 表单"))?;
    save_input(cx, input).await
}

#[route(POST "/ui/providers/save")]
pub async fn save(cx: &Cx, Form(input): Form<ProviderForm>) -> Result<Json<Outcome>> {
    Ok(Json(save_input(cx, input).await?))
}

async fn save_input(cx: &Cx, mut input: ProviderForm) -> Result<Outcome> {
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let store = &state.store;
    let record_id = input.id.as_deref().and_then(|id| id.parse::<i64>().ok());
    let result: std::result::Result<ProviderView, StoreError> = async {
        let status = ProbeStatus::parse(&input.models_probe_status)?;
        let provider = input.input().map_err(StoreError::Validation)?;
        let saved = match input.id.as_deref() {
            Some(id) => match (
                id.parse::<i64>(),
                input.version.as_deref().unwrap_or("").parse::<u64>(),
            ) {
                (Ok(id), Ok(version)) => {
                    store
                        .update_with_probe_status(id, version, provider, Some(status))
                        .await
                }
                _ => Err(StoreError::Validation(
                    "表单版本无效，请重新打开编辑页".to_owned(),
                )),
            },
            None => store.create_with_probe_status(provider, status).await,
        }?;
        Ok(saved)
    }
    .await;
    input.api_key.clear();
    state.telemetry.provider_operation(
        if input.id.is_some() {
            "update"
        } else {
            "create"
        },
        result
            .as_ref()
            .ok()
            .map(|provider| provider.id)
            .or(record_id),
        Some(
            result
                .as_ref()
                .map(|provider| provider.name.as_str())
                .unwrap_or(&input.name),
        ),
        result.as_ref().err(),
    );
    Ok(result
        .map(|provider| {
            let name = provider.name;
            format!(
                "「{name}」{}",
                if input.id.is_some() {
                    "已保存"
                } else {
                    "已创建"
                }
            )
        })
        .map_err(|error| format!("「{}」保存失败：{error}", input.name)))
}

#[derive(Deserialize)]
pub struct ActionForm {
    #[serde(default)]
    csrf: String,
    id: i64,
    version: u64,
    action: String,
    #[serde(default)]
    protocol: String,
}

#[procedure]
pub async fn provider_action(
    cx: &Cx,
    csrf: String,
    id: String,
    version: String,
    action: String,
    protocol: String,
) -> Result<Outcome> {
    action_input(
        cx,
        ActionForm {
            csrf,
            id: id.parse()?,
            version: version.parse()?,
            action,
            protocol,
        },
    )
    .await
}

#[route(POST "/ui/providers/action")]
pub async fn perform_action(cx: &Cx, Form(input): Form<ActionForm>) -> Result<Json<Outcome>> {
    Ok(Json(action_input(cx, input).await?))
}

async fn action_input(cx: &Cx, input: ActionForm) -> Result<Outcome> {
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let store = &state.store;
    let (action, label, completed) = match input.action.as_str() {
        "activate" => ("activate", "设为当前服务", "已设为当前服务"),
        "enable" => ("enable", "启用", "已启用"),
        "disable" => ("disable", "停用", "已停用"),
        "delete" => ("delete", "删除", "已删除"),
        _ => return Err(topcoat::router::error::bad_request("无效的操作").into()),
    };
    let name = match store.get(input.id).await {
        Ok(provider) => provider.name,
        Err(error) => {
            state
                .telemetry
                .provider_operation(action, Some(input.id), None, Some(&error));
            return Err(error.into());
        }
    };
    let result = match input.action.as_str() {
        "activate" => match parse_protocol(&input.protocol) {
            Ok(protocol) => store.activate(input.id, input.version, protocol).await,
            Err(error) => Err(StoreError::Validation(error)),
        },
        "enable" => store
            .set_enabled(input.id, input.version, true)
            .await
            .map(|_| ()),
        "disable" => store
            .set_enabled(input.id, input.version, false)
            .await
            .map(|_| ()),
        "delete" => store.delete(input.id, input.version).await.map(|_| ()),
        _ => unreachable!(),
    };
    state
        .telemetry
        .provider_operation(action, Some(input.id), Some(&name), result.as_ref().err());
    let protocol_name = if input.action == "activate" {
        parse_protocol(&input.protocol)
            .map(protocol_label)
            .unwrap_or("")
    } else {
        ""
    };
    Ok(result
        .map(|_| {
            if protocol_name.is_empty() {
                format!("「{name}」{completed}")
            } else {
                format!("「{name}」{protocol_name} {completed}")
            }
        })
        .map_err(|error| format!("「{name}」{label}失败：{error}")))
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
    protocol: &str,
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
        // Native procedures have no Rust expression API for transport rejections in 0.8.
        let result = raw!("await Promise.resolve(${provider_action}.call(${csrf}, ${id}, ${version}, ${action}, ${protocol})).catch(() => ${unavailable})", unavailable.clone());
        busy.set(false);
        if result.is_ok() {
            success.set(result.unwrap());
            refresh.increment();
        } else { failure.set(result.unwrap_err()); }
    }) }
}

// Business state stays in Topcoat. IDs and versions remain strings in the
// browser so database integer values are never rounded through JavaScript f64.
struct EditorSignals {
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
    upstream_url: Signal<String>,
    enabled: Signal<bool>,
    api_key: Signal<String>,
    models_path: Signal<String>,
    models_protocol: Signal<String>,
    models_probe_status: Signal<String>,
    models_probe_message: Signal<String>,
    anthropic_version: Signal<String>,
    connect_timeout: Signal<String>,
    read_timeout: Signal<String>,
    write_timeout: Signal<String>,
}

impl EditorSignals {
    fn new(cx: &Cx, input: &ProviderForm, error: Option<&str>, open: bool) -> Self {
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
            upstream_url: signal(cx, || input.upstream_url.clone()),
            enabled: signal(cx, || input.enabled),
            // Never initialize browser state from a submitted or stored secret.
            api_key: signal(cx, String::new),
            models_path: signal(cx, || input.models_path.clone()),
            models_protocol: signal(cx, || input.models_protocol.clone()),
            models_probe_status: signal(cx, || input.models_probe_status.clone()),
            models_probe_message: signal(cx, String::new),
            anthropic_version: signal(cx, || input.anthropic_version.clone()),
            connect_timeout: signal(cx, || input.connect_timeout_ms.clone()),
            read_timeout: signal(cx, || input.read_timeout_ms.clone()),
            write_timeout: signal(cx, || input.write_timeout_ms.clone()),
        }
    }
}

fn editor_trigger(cx: &Cx, editor: &EditorSignals, input: ProviderForm) -> Attributes {
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
        upstream_url,
        enabled,
        api_key,
        models_path,
        models_protocol,
        models_probe_status,
        models_probe_message,
        anthropic_version,
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
    let initial_upstream_url = input.upstream_url;
    let initial_enabled = input.enabled;
    let initial_anthropic = input.anthropic_version;
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
            upstream_url.set(initial_upstream_url.to_owned());
            enabled.set(initial_enabled);
            anthropic_version.set(initial_anthropic.to_owned());
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
async fn provider_editor(
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
        upstream_url,
        enabled,
        api_key,
        models_path,
        models_protocol,
        models_probe_status,
        models_probe_message,
        anthropic_version,
        connect_timeout,
        read_timeout,
        write_timeout,
    } = editor;
    let close = dialog_close_attributes(cx, "provider-dialog");
    let success = &controls.success;
    let failure = &controls.failure;
    let refresh = &controls.refresh;
    let unavailable: Outcome = Err("保存请求失败或结果未确认，请检查列表状态后重试".to_owned());
    let probe_unavailable: Outcome = Err("模型探测请求失败，请稍后重试".to_owned());
    Ok(view! {
        dialog(config: DialogConfig::new("provider-dialog", "Provider 配置"),
            open: Some(open), title: Some(title), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { cx => class="w-[min(720px,calc(100%_-_32px))]! max-[640px]:w-[calc(100%_-_24px)]! max-[640px]:max-h-[calc(100dvh_-_24px)]! [&_.gr-dialog-header]:px-6 [&_.gr-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg max-[640px]:[&_.gr-dialog-header]:px-4 max-[640px]:[&_.gr-dialog-header]:py-4" @close=$(|_event: Event| api_key.set("".to_owned())) },
            <form class="m-0 flex min-h-0 flex-col" action="/ui/providers/save" method="post" autocomplete="off"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    busy.set(true);
                    error.set("".to_owned());
                    success.set("".to_owned());
                    failure.set("".to_owned());
                    let result = raw!("await Promise.resolve(${save_provider}.call(cx.hydrate(JSON.stringify({csrf:${csrf}.dehydrate(),id:${id}.get().dehydrate()||null,version:${version}.get().dehydrate()||null,name:${name}.get().dehydrate(),openai_chat:${openai_chat}.get().dehydrate(),openai_chat_path:${openai_chat_path}.get().dehydrate(),openai_responses:${openai_responses}.get().dehydrate(),openai_responses_path:${openai_responses_path}.get().dehydrate(),anthropic_messages:${anthropic_messages}.get().dehydrate(),anthropic_messages_path:${anthropic_messages_path}.get().dehydrate(),upstream_url:${upstream_url}.get().dehydrate(),enabled:${enabled}.get().dehydrate(),api_key:${api_key}.get().dehydrate(),models_path:${models_path}.get().dehydrate(),models_protocol:${models_protocol}.get().dehydrate(),models_probe_status:${models_probe_status}.get().dehydrate(),anthropic_version:${anthropic_version}.get().dehydrate(),connect_timeout_ms:${connect_timeout}.get().dehydrate(),read_timeout_ms:${read_timeout}.get().dehydrate(),write_timeout_ms:${write_timeout}.get().dehydrate()})))).catch(() => ${unavailable})", unavailable.clone());
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
                        <div class="mt-5 flex items-start gap-2 text-sm leading-[22px] text-heading [&_small]:ml-3 [&_small]:inline [&_small]:text-[13px] [&_small]:text-muted max-[640px]:[&_small]:ml-0 max-[640px]:[&_small]:block"><input type="hidden" name="enabled" :value=$(if enabled.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if enabled.get() { "true" } else { "false" }) @click=$(|_event: Event| enabled.toggle())><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span><span>"启用此 Provider"</span></button><small>"保存后可在列表中设为当前服务。"</small></div>
                    </section>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-6 [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"接口协议与上游路径"</h3>
                        <p class=(FIELD_HINT)>"至少选择一个协议。路径只填写上游接口路径。"</p>
                        <div class="mt-4 grid gap-4">
                            <div class="grid grid-cols-[190px_1fr] items-center gap-3 max-[640px]:grid-cols-1"><input type="hidden" name="openai_chat" :value=$(if openai_chat.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if openai_chat.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !openai_chat.get(); openai_chat.set(selected); if !selected { if models_protocol.get() == "openai_chat" { models_protocol.set(if openai_responses.get() { "openai_responses" } else if anthropic_messages.get() { "anthropic_messages" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("openai_chat".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"OpenAI Chat"</button><input class="w-full" name="openai_chat_path" aria-label="OpenAI Chat 上游路径" :value=$(openai_chat_path.get()) @input=$(|event: Event| openai_chat_path.set(event.target.value)) :disabled=$(!openai_chat.get()) :required=$(openai_chat.get())></div>
                            <div class="grid grid-cols-[190px_1fr] items-center gap-3 max-[640px]:grid-cols-1"><input type="hidden" name="openai_responses" :value=$(if openai_responses.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if openai_responses.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !openai_responses.get(); openai_responses.set(selected); if !selected { if models_protocol.get() == "openai_responses" { models_protocol.set(if openai_chat.get() { "openai_chat" } else if anthropic_messages.get() { "anthropic_messages" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("openai_responses".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"OpenAI Responses"</button><input class="w-full" name="openai_responses_path" aria-label="OpenAI Responses 上游路径" :value=$(openai_responses_path.get()) @input=$(|event: Event| openai_responses_path.set(event.target.value)) :disabled=$(!openai_responses.get()) :required=$(openai_responses.get())></div>
                            <div class="grid grid-cols-[190px_minmax(0,1fr)] items-end gap-3 max-[640px]:grid-cols-1"><input type="hidden" name="anthropic_messages" :value=$(if anthropic_messages.get() { "true" } else { "false" })><button class=(CHECKBOX) type="button" role="checkbox" :aria-checked=$(if anthropic_messages.get() { "true" } else { "false" }) @click=$(|_event: Event| { let selected = !anthropic_messages.get(); anthropic_messages.set(selected); if !selected { if models_protocol.get() == "anthropic_messages" { models_protocol.set(if openai_chat.get() { "openai_chat" } else if openai_responses.get() { "openai_responses" } else { "" }.to_owned()); } } else if models_protocol.get().is_empty() { models_protocol.set("anthropic_messages".to_owned()); } models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); })><span class=(CHECKBOX_MARK) aria-hidden="true"><span class="invisible group-aria-[checked=true]:visible">"✓"</span></span>"Anthropic Messages"</button><div class="flex min-w-0 items-end gap-3 max-[640px]:flex-col max-[640px]:items-stretch"><input class="min-w-0 flex-1 max-[640px]:w-full" name="anthropic_messages_path" aria-label="Anthropic Messages 上游路径" :value=$(anthropic_messages_path.get()) @input=$(|event: Event| anthropic_messages_path.set(event.target.value)) :disabled=$(!anthropic_messages.get()) :required=$(anthropic_messages.get())><div class="w-[160px] shrink-0 max-[640px]:w-full" :hidden=$(!anthropic_messages.get())>form_field(config: FormFieldConfig::new("anthropic-version", "API 版本"), <input class="w-full" id="anthropic-version" name="anthropic_version" :value=$(anthropic_version.get()) @input=$(|event: Event| { anthropic_version.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) placeholder="2023-06-01" :disabled=$(!anthropic_messages.get())>)</div></div></div>
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
                            <div class="max-[640px]:col-span-full max-[640px]:row-start-2">form_field(config: FormFieldConfig::new("models-protocol", "探测协议").required(), <select id="models-protocol" name="models_protocol" :value=$(models_protocol.get()) @change=$(|event: Event| { models_protocol.set(event.target.value); models_probe_status.set("unprobed".to_owned()); models_probe_message.set("".to_owned()); }) required=""><option value="" disabled="">"请选择协议"</option><option value="openai_chat" :disabled=$(!openai_chat.get())>"OpenAI Chat"</option><option value="openai_responses" :disabled=$(!openai_responses.get())>"OpenAI Responses"</option><option value="anthropic_messages" :disabled=$(!anthropic_messages.get())>"Anthropic Messages"</option></select>)</div>
                            <button class=(BUTTON) type="button" :disabled=$(busy.get()) @click=$(async |_event: Event| {
                                if busy.get() { return; }
                                busy.set(true);
                                models_probe_message.set("".to_owned());
                                let result = raw!("await Promise.resolve(${preview_models}.call(cx.hydrate(JSON.stringify({csrf:${csrf}.dehydrate(),id:${id}.get().dehydrate()||null,version:${version}.get().dehydrate()||null,name:${name}.get().dehydrate(),openai_chat:${openai_chat}.get().dehydrate(),openai_chat_path:${openai_chat_path}.get().dehydrate(),openai_responses:${openai_responses}.get().dehydrate(),openai_responses_path:${openai_responses_path}.get().dehydrate(),anthropic_messages:${anthropic_messages}.get().dehydrate(),anthropic_messages_path:${anthropic_messages_path}.get().dehydrate(),upstream_url:${upstream_url}.get().dehydrate(),enabled:${enabled}.get().dehydrate(),api_key:${api_key}.get().dehydrate(),models_path:${models_path}.get().dehydrate(),models_protocol:${models_protocol}.get().dehydrate(),models_probe_status:${models_probe_status}.get().dehydrate(),anthropic_version:${anthropic_version}.get().dehydrate(),connect_timeout_ms:${connect_timeout}.get().dehydrate(),read_timeout_ms:${read_timeout}.get().dehydrate(),write_timeout_ms:${write_timeout}.get().dehydrate()})))).catch(() => ${probe_unavailable})", probe_unavailable.clone());
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
