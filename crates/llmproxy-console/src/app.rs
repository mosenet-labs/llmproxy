use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ProviderInput, ProviderStore, ProviderView};
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
        request::{headers, original_uri},
        response::Response,
        route,
    },
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, class, component, view},
};
use topcoat_ant_design::icons::{
    APARTMENT_OUTLINED, APPSTORE_OUTLINED, ARROW_RIGHT_OUTLINED, CHECK_CIRCLE_FILLED,
    DOWN_OUTLINED, INFO_CIRCLE_FILLED, PLUS_OUTLINED, SEARCH_OUTLINED,
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
}

// Complete class names let Tailwind discover styles in Rust at build time.
const PAGE_HEADING: &str = "mb-6 flex min-h-20 items-center justify-between gap-6 max-[640px]:min-h-0 max-[640px]:flex-col max-[640px]:items-start max-[640px]:gap-4 [&_h1]:m-0 [&_h1]:text-[28px] [&_h1]:font-semibold [&_h1]:leading-[1.35] max-[640px]:[&_h1]:text-2xl [&_p]:mt-2 [&_p]:mb-0 [&_p]:text-sm [&_p]:leading-relaxed [&_p]:text-secondary";
const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-control-border bg-white px-4 text-sm font-medium leading-5 text-heading hover:border-primary-hover hover:text-primary";
const PRIMARY_BUTTON: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const TEXT_LINK: &str = "border-0 bg-transparent p-0 text-sm leading-[22px] whitespace-nowrap text-primary hover:text-primary-hover";
const NAV_ITEM: &str = "flex min-h-11 items-center gap-3 whitespace-nowrap rounded-md px-3 py-2.5 text-sm text-secondary hover:bg-surface hover:text-primary aria-[current=page]:bg-primary-soft aria-[current=page]:font-medium aria-[current=page]:text-[#0958d9] max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-[13px]";
const FIELDS_GRID: &str = "grid grid-cols-2 items-start gap-5 max-[640px]:grid-cols-1 [&>div]:content-start [&_input:not([type=checkbox])]:w-full [&_select]:w-full [&_label]:text-sm [&_.text-xs]:text-[13px] [&_.text-xs]:leading-relaxed [&_.text-xs]:text-secondary";
const FIELD_HINT: &str = "mt-2 mb-0 text-[13px] leading-relaxed text-secondary";

const PROTOCOLS: [Protocol; 3] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
];

#[route(GET "/providers")]
pub async fn providers_redirect() -> Result<topcoat::router::error::SeeOther> {
    Ok(see_other("/"))
}

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "OpenAI Chat",
        Protocol::OpenAiResponses => "OpenAI Responses",
        Protocol::AnthropicMessages => "Anthropic Messages",
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
    let routes_page = original_uri(cx).path() == "/routes";
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
                <link rel="stylesheet" href="/assets/console.css">
                topcoat::runtime::script()
            </head>
            <body>
                <div class="grid min-h-screen grid-cols-[216px_minmax(0,1fr)] max-[900px]:grid-cols-[176px_minmax(0,1fr)] max-[640px]:block">
                    <aside class="sticky top-0 flex h-screen flex-col border-r border-border bg-white px-3 max-[640px]:static max-[640px]:h-auto max-[640px]:border-r-0 max-[640px]:border-b max-[640px]:px-4 max-[640px]:pb-2">
                        <a class="flex h-16 shrink-0 items-center gap-3 px-3 text-lg font-semibold max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-base max-[640px]:h-14 max-[640px]:px-0" href="/"><span class="grid size-8 place-items-center rounded-lg bg-primary text-xl font-bold text-white" aria-hidden="true">"L"</span><strong>"LLMProxy"</strong></a>
                        <nav class="mt-4 grid gap-1 max-[640px]:mt-0 max-[640px]:grid-cols-2" aria-label="主导航">
                            <a class=(NAV_ITEM) href="/" aria-current=(if routes_page { None } else { Some("page") })>icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"Provider 管理"</a>
                            <a class=(NAV_ITEM) href="/routes" aria-current=(if routes_page { Some("page") } else { None })>icon(data: APARTMENT_OUTLINED, attrs: attributes! { class="size-[18px] shrink-0" aria-hidden="true" })"路由概览"</a>
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

#[page("/")]
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

#[page("/routes")]
pub async fn routes(cx: &Cx) -> Result<impl View> {
    let all = app_context::<AppState>(cx).store.list().await?;
    Ok(view! {
        <section class=(PAGE_HEADING)>
            <div><h1>"路由概览"</h1><p>"查看各协议入口与当前使用的 Provider。"</p></div>
            <a class=(BUTTON) href="/">"管理 Provider"</a>
        </section>
        <section class="grid grid-cols-3 gap-5 max-[1180px]:grid-cols-1" id="routes" aria-label="三个协议的当前 Provider">
            for protocol in PROTOCOLS {
                <a class="group min-w-0 rounded-lg border border-border bg-white p-6 shadow-xs hover:border-[#91caff] max-[640px]:p-5" href=(format!("/?protocol={}", protocol.as_str()))>
                    <div class="flex items-center gap-3"><span class="grid size-8 shrink-0 place-items-center rounded-md bg-primary-soft text-sm font-semibold text-primary" aria-hidden="true">(match protocol { Protocol::OpenAiChat => "C", Protocol::OpenAiResponses => "R", Protocol::AnthropicMessages => "A" })</span><span class="text-sm font-medium text-heading">(protocol_label(protocol))</span></div>
                    if let Some(provider) = all.iter().find(|provider| provider.protocol == protocol && provider.active) {
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
                && (query.protocol.is_empty() || provider.protocol.as_str() == query.protocol)
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
            <form class="flex flex-wrap items-center gap-3 px-6 py-5 max-[640px]:gap-2 max-[640px]:px-4 [&_select]:h-9 [&_select]:min-w-[144px] [&_select]:text-sm max-[640px]:[&_select]:min-w-0 max-[640px]:[&_select]:flex-1" method="get" action="/" role="search">
                <div class="flex h-9 w-[300px] items-center gap-2 rounded-md border border-control-border pl-3 focus-within:border-primary-hover focus-within:ring-2 focus-within:ring-primary/10 max-[640px]:w-full [&_input]:h-8 [&_input]:w-full [&_input]:border-0 [&_input]:bg-transparent [&_input]:pl-0 [&_input]:text-sm [&_input]:shadow-none">icon(data: SEARCH_OUTLINED, attrs: attributes! { class="size-4 shrink-0 text-muted" aria-hidden="true" })<input aria-label="搜索名称或主机" name="q" value=(query.q.as_str()) placeholder="搜索名称或主机地址"></div>
                <select name="protocol" aria-label="筛选协议"><option value="">"全部协议"</option>for protocol in PROTOCOLS { <option value=(protocol.as_str()) selected=(query.protocol == protocol.as_str())>(protocol_label(protocol))</option> }</select>
                <select name="state" aria-label="筛选状态"><option value="">"全部状态"</option><option value="enabled" selected=(query.state == "enabled")>"已启用"</option><option value="disabled" selected=(query.state == "disabled")>"已停用"</option><option value="active" selected=(query.state == "active")>"当前使用"</option></select>
                <button class=(BUTTON) type="submit">"查询"</button>
                if !query.q.is_empty() || !query.protocol.is_empty() || !query.state.is_empty() { <a class=(class!(TEXT_LINK, "px-1")) href="/">"重置"</a> }
            </form>
            if providers.is_empty() {
                <div class="border-t border-border px-6 py-12 text-center [&_h3]:mt-4 [&_h3]:mb-2 [&_h3]:text-base [&_h3]:font-medium [&_h3]:text-heading [&_p]:mt-0 [&_p]:mb-6 [&_p]:text-sm [&_p]:text-secondary">icon(data: APPSTORE_OUTLINED, attrs: attributes! { class="mx-auto block size-10 text-[#bfbfbf]" aria-hidden="true" })<h3>(if all.is_empty() { "连接第一个模型服务" } else { "没有找到匹配的 Provider" })</h3><p>(if all.is_empty() { "添加上游地址与 API Key，即可开始管理你的模型连接。" } else { "尝试调整搜索关键词，或清除筛选条件。" })</p>if all.is_empty() { <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())>"新建 Provider"</button> } else { <a class=(class!(BUTTON, PRIMARY_BUTTON)) href="/">"清除筛选"</a> }</div>
            } else {
                data_table(label: "Provider 列表", attrs: attributes! { class="min-w-[900px] [&_th]:px-6! [&_th]:text-[13px]! [&_td]:px-6! [&_td]:py-4! [&_td]:text-sm! [&_.gr-tag]:text-[13px]" },
                    <thead><tr><th>"名称 / 协议"</th><th>"上游地址"</th><th>"状态"</th><th>"凭据"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        for provider in &providers {
                            <tr id=(format!("provider-{}", provider.id))>
                                <td><button class="block border-0 bg-transparent p-0 text-left text-sm font-medium leading-[22px] text-heading hover:text-primary" type="button" (editor_trigger(cx, &editor, provider.clone().into()))>(provider.name.as_str())</button><span class="mt-1 block text-[13px] leading-5 text-secondary">(protocol_label(provider.protocol))</span></td>
                                <td><span class="whitespace-nowrap text-sm text-heading">(provider_url(provider))</span><span class="mt-1 block text-[13px] leading-5 text-secondary">"读取超时 "(provider.read_timeout_ms / 1000)" 秒"</span></td>
                                <td><div class="flex max-w-[155px] flex-wrap gap-[5px]">tag(tone: if provider.enabled { TagTone::Success } else { TagTone::Default }, (if provider.enabled { "已启用" } else { "已停用" })) if provider.active { tag(tone: TagTone::Processing, "当前使用") }</div></td>
                                <td><span class="inline-flex items-center gap-1.5 whitespace-nowrap text-[13px] text-secondary">if provider.key_configured { icon(data: CHECK_CIRCLE_FILLED, attrs: attributes! { class="size-3.5 text-muted" aria-hidden="true" }) }(if provider.key_configured { "已配置" } else { "未配置" })</span></td>
                                <td><div class="flex min-w-[125px] items-center justify-end gap-3">
                                    <button class=(TEXT_LINK) type="button" (editor_trigger(cx, &editor, provider.clone().into()))>"编辑"</button>
                                    if provider.enabled && !provider.active {
                                        action_form(controls: &controls, csrf: csrf, provider: provider, action: "activate", label: "设为当前服务")
                                    }
                                    if provider.enabled {
                                        provider_action_confirmation(controls: &controls, provider: provider, csrf: csrf, delete: false)
                                    } else {
                                        action_form(controls: &controls, csrf: csrf, provider: provider, action: "enable", label: "启用")
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
    let submit = action_submit(cx, controls, csrf, provider, action);
    let busy = &controls.busy;
    Ok(view! {
        <button class=(class!(TEXT_LINK, "text-[#cf1322]! hover:text-[#ff4d4f]!" if delete)) type="button" (trigger) :disabled=$(busy.get())>(label)</button>
        popconfirm(id: id.as_str(), title: title.as_str(), language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="[&_footer]:items-center [&_footer_.gr-button]:h-8! [&_footer_.gr-button]:w-[88px]! [&_footer_.gr-button]:px-3! [&_footer_.gr-button]:py-1! [&_footer_.gr-button]:text-sm! [&_footer_.gr-button]:leading-[22px]!" },
            <form class="m-0 inline-flex" action="/providers/action" method="post" (submit)><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><button class="gr-button gr-button-danger" type="submit" :disabled=$(busy.get())>(format!("确认{label}"))</button></form>
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
    label: &str,
) -> Result<impl View> {
    let submit = action_submit(cx, controls, csrf, provider, action);
    let busy = &controls.busy;
    Ok(
        view! { <form class="m-0 inline-flex" action="/providers/action" method="post" (submit)><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><button class=(TEXT_LINK) type="submit" :disabled=$(busy.get())>(label)</button></form> },
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
    id: Option<i64>,
    version: Option<u64>,
    name: String,
    protocol: String,
    upstream_url: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    anthropic_version: String,
    connect_timeout_ms: String,
    read_timeout_ms: String,
    write_timeout_ms: String,
}

impl Default for ProviderForm {
    fn default() -> Self {
        Self {
            csrf: String::new(),
            id: None,
            version: None,
            name: String::new(),
            protocol: "openai_chat".to_owned(),
            upstream_url: String::new(),
            enabled: true,
            api_key: String::new(),
            anthropic_version: "2023-06-01".to_owned(),
            connect_timeout_ms: "10000".to_owned(),
            read_timeout_ms: "60000".to_owned(),
            write_timeout_ms: "30000".to_owned(),
        }
    }
}

impl From<ProviderView> for ProviderForm {
    fn from(provider: ProviderView) -> Self {
        Self {
            upstream_url: provider_url(&provider),
            csrf: String::new(),
            id: Some(provider.id),
            version: Some(provider.version),
            name: provider.name,
            protocol: provider.protocol.as_str().to_owned(),
            enabled: provider.enabled,
            api_key: String::new(),
            anthropic_version: provider.anthropic_version.unwrap_or_default(),
            connect_timeout_ms: provider.connect_timeout_ms.to_string(),
            read_timeout_ms: provider.read_timeout_ms.to_string(),
            write_timeout_ms: provider.write_timeout_ms.to_string(),
        }
    }
}

impl ProviderForm {
    fn input(&self) -> std::result::Result<ProviderInput, String> {
        let (host, port, tls) = parse_upstream_url(&self.upstream_url)?;
        let timeout = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| "超时必须是以毫秒为单位的正整数".to_owned())
        };
        Ok(ProviderInput {
            name: self.name.clone(),
            protocol: parse_protocol(&self.protocol)?,
            host,
            port,
            tls,
            enabled: self.enabled,
            api_key: self.api_key.clone(),
            anthropic_version: if self.protocol != "anthropic_messages"
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

#[page("/providers/form")]
pub async fn form(cx: &Cx, Form(query): Form<EditQuery>) -> Result<impl View> {
    let _ = cx;
    let filters = ListQuery::default();
    let edit_id = query.id.map(|id| id.to_string()).unwrap_or_default();
    Ok(view! { provider_workspace(query: &filters, edit_id: &edit_id, editor_open: true) })
}

type Outcome = std::result::Result<String, String>;

#[procedure]
pub async fn save_provider(
    cx: &Cx,
    csrf: String,
    id: String,
    version: String,
    name: String,
    protocol: String,
    upstream_url: String,
    enabled: bool,
    api_key: String,
    anthropic_version: String,
    connect_timeout_ms: String,
    read_timeout_ms: String,
    write_timeout_ms: String,
) -> Result<Outcome> {
    let id = if id.is_empty() {
        None
    } else {
        Some(id.parse::<i64>()?)
    };
    let version = if version.is_empty() {
        None
    } else {
        Some(version.parse::<u64>()?)
    };
    save_input(
        cx,
        ProviderForm {
            csrf,
            id,
            version,
            name,
            protocol,
            upstream_url,
            enabled,
            api_key,
            anthropic_version,
            connect_timeout_ms,
            read_timeout_ms,
            write_timeout_ms,
        },
    )
    .await
}

#[route(POST "/providers/save")]
pub async fn save(cx: &Cx, Form(input): Form<ProviderForm>) -> Result<Json<Outcome>> {
    Ok(Json(save_input(cx, input).await?))
}

async fn save_input(cx: &Cx, mut input: ProviderForm) -> Result<Outcome> {
    check_csrf(cx, &input.csrf)?;
    let store = &app_context::<AppState>(cx).store;
    let result = match input.input() {
        Err(error) => Err(error),
        Ok(provider) => match input.id {
            Some(id) => match input.version {
                Some(version) => store
                    .update(id, version, provider)
                    .await
                    .map(|provider| provider.name)
                    .map_err(|error| error.to_string()),
                None => Err("表单版本缺失，请重新打开编辑页".to_owned()),
            },
            None => store
                .create(provider)
                .await
                .map(|provider| provider.name)
                .map_err(|error| error.to_string()),
        },
    };
    input.api_key.clear();
    Ok(result
        .map(|name| {
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
}

#[procedure]
pub async fn provider_action(
    cx: &Cx,
    csrf: String,
    id: String,
    version: String,
    action: String,
) -> Result<Outcome> {
    action_input(
        cx,
        ActionForm {
            csrf,
            id: id.parse()?,
            version: version.parse()?,
            action,
        },
    )
    .await
}

#[route(POST "/providers/action")]
pub async fn perform_action(cx: &Cx, Form(input): Form<ActionForm>) -> Result<Json<Outcome>> {
    Ok(Json(action_input(cx, input).await?))
}

async fn action_input(cx: &Cx, input: ActionForm) -> Result<Outcome> {
    check_csrf(cx, &input.csrf)?;
    let store = &app_context::<AppState>(cx).store;
    let (label, completed) = match input.action.as_str() {
        "activate" => ("设为当前服务", "已设为当前服务"),
        "enable" => ("启用", "已启用"),
        "disable" => ("停用", "已停用"),
        "delete" => ("删除", "已删除"),
        _ => return Err(topcoat::router::error::bad_request("无效的操作").into()),
    };
    let name = store.get(input.id).await?.name;
    let result = match input.action.as_str() {
        "activate" => store.activate(input.id, input.version).await.map(|_| ()),
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
    Ok(result
        .map(|_| format!("「{name}」{completed}"))
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
        let result = raw!("await Promise.resolve(${provider_action}.call(${csrf}, ${id}, ${version}, ${action})).catch(() => ${unavailable})", unavailable.clone());
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
    protocol: Signal<String>,
    upstream_url: Signal<String>,
    enabled: Signal<bool>,
    api_key: Signal<String>,
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
            id: signal(cx, || input.id.map(|id| id.to_string()).unwrap_or_default()),
            version: signal(cx, || {
                input
                    .version
                    .map(|version| version.to_string())
                    .unwrap_or_default()
            }),
            name: signal(cx, || input.name.clone()),
            protocol: signal(cx, || input.protocol.clone()),
            upstream_url: signal(cx, || input.upstream_url.clone()),
            enabled: signal(cx, || input.enabled),
            // Never initialize browser state from a submitted or stored secret.
            api_key: signal(cx, String::new),
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
        protocol,
        upstream_url,
        enabled,
        api_key,
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
    let initial_id = input.id.map(|id| id.to_string()).unwrap_or_default();
    let initial_version = input
        .version
        .map(|version| version.to_string())
        .unwrap_or_default();
    let initial_name = input.name;
    let initial_protocol = input.protocol;
    let initial_upstream_url = input.upstream_url;
    let initial_enabled = input.enabled;
    let initial_anthropic = input.anthropic_version;
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
            protocol.set(initial_protocol.to_owned());
            upstream_url.set(initial_upstream_url.to_owned());
            enabled.set(initial_enabled);
            anthropic_version.set(initial_anthropic.to_owned());
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
        protocol,
        upstream_url,
        enabled,
        api_key,
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
    Ok(view! {
        dialog(config: DialogConfig::new("provider-dialog", "Provider 配置"),
            open: Some(open), title: Some(title), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { cx => class="w-[min(720px,calc(100%_-_32px))]! max-[640px]:w-[calc(100%_-_24px)]! max-[640px]:max-h-[calc(100dvh_-_24px)]! [&_.gr-dialog-header]:px-6 [&_.gr-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg max-[640px]:[&_.gr-dialog-header]:px-4 max-[640px]:[&_.gr-dialog-header]:py-4" @close=$(|_event: Event| api_key.set("".to_owned())) },
            <form class="m-0 flex min-h-0 flex-col" action="/providers/save" method="post" autocomplete="off"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    busy.set(true);
                    error.set("".to_owned());
                    success.set("".to_owned());
                    failure.set("".to_owned());
                    let result = raw!("await Promise.resolve(${save_provider}.call(${csrf}, ${id}.get(), ${version}.get(), ${name}.get(), ${protocol}.get(), ${upstream_url}.get(), ${enabled}.get(), ${api_key}.get(), ${anthropic_version}.get(), ${connect_timeout}.get(), ${read_timeout}.get(), ${write_timeout}.get())).catch(() => ${unavailable})", unavailable.clone());
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
                            form_field(config: FormFieldConfig::new("protocol", "接口协议").required(), <select id="protocol" name="protocol" :value=$(protocol.get()) @change=$(|event: Event| protocol.set(event.target.value))>for choice in PROTOCOLS { <option value=(choice.as_str())>(protocol_label(choice))</option> }</select>)
                        </div>
                        <label class="mt-5 flex cursor-pointer items-start gap-2 text-sm leading-[22px] text-heading [&_small]:ml-3 [&_small]:inline [&_small]:text-[13px] [&_small]:text-muted max-[640px]:[&_small]:ml-0 max-[640px]:[&_small]:block"><input type="checkbox" name="enabled" value="true" :checked=$(enabled.get()) @change=$(|event: Event| enabled.set(event.target.checked))><span>"启用此 Provider"<small>"保存后可在列表中设为当前服务。"</small></span></label>
                    </section>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-6 [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"连接与凭据"</h3>
                        <div class=(FIELDS_GRID)>
                            <div class="col-span-full">form_field(config: FormFieldConfig::new("upstream-url", "上游地址").required(), <input id="upstream-url" name="upstream_url" type="url" :value=$(upstream_url.get()) @input=$(|event: Event| upstream_url.set(event.target.value)) placeholder="https://api.deepseek.com" required="" aria-describedby="connection-help">)</div>
                        </div>
                        <p class=(FIELD_HINT) id="connection-help">"例如 https://api.deepseek.com；本地服务可填写 http://127.0.0.1:11434。"</p>
                        <div class=(class!(FIELDS_GRID, "mt-5"))>
                            <div class="col-span-full">form_field(config: FormFieldConfig::new("api-key", "API Key"),
                                <input id="api-key" name="api_key" type="password" autocomplete="new-password" :value=$(if open.get() { api_key.get() } else { "".to_owned() }) @input=$(|event: Event| api_key.set(event.target.value)) :placeholder=$(if id.get().is_empty() { "输入 Provider API Key" } else { "留空以保留现有 API Key" }) :required=$(id.get().is_empty()) aria-describedby="api-key-help">
                                <p class=(FIELD_HINT) id="api-key-help">$(if id.get().is_empty() { "必填，凭据加密保存且不会回显。" } else { "留空保留现有凭据，输入新值即可更换。" })</p>
                            )</div>
                            <div class="col-span-full" :hidden=$(protocol.get() != "anthropic_messages")>form_field(config: FormFieldConfig::new("anthropic-version", "Anthropic API 版本").with_hint("请求上游时使用的 anthropic-version。"), <input id="anthropic-version" name="anthropic_version" :value=$(anthropic_version.get()) @input=$(|event: Event| anthropic_version.set(event.target.value)) placeholder="2023-06-01" :disabled=$(protocol.get() != "anthropic_messages") aria-describedby="anthropic-version-help">)</div>
                        </div>
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
