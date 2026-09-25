use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ProviderInput, ProviderStore, ProviderView};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
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
const PAGE_HEADING: &str = "mb-[26px] flex items-center justify-between gap-6 max-[900px]:items-start max-[640px]:mb-[21px] max-[640px]:gap-3 [&_h1]:m-0 [&_h1]:text-[25px] [&_h1]:font-semibold [&_h1]:leading-[1.4] [&_h1]:tracking-[-.5px] max-[900px]:[&_h1]:text-[23px] max-[640px]:[&_h1]:text-[21px] [&_p]:mt-2.5 [&_p]:mb-0 [&_p]:text-[13px] [&_p]:leading-relaxed [&_p]:text-muted max-[900px]:[&_p]:max-w-[340px] max-[640px]:[&_p]:mt-2 max-[640px]:[&_p]:max-w-[230px] max-[640px]:[&_p]:text-[11px] max-[900px]:[&>button]:mt-2.5 max-[900px]:[&>a]:mt-2.5 max-[640px]:[&>button]:min-h-[33px] max-[640px]:[&>button]:px-2.5 max-[640px]:[&>button]:py-1.5 max-[640px]:[&>button]:text-[11px] max-[640px]:[&>a]:text-[11px]";
const BUTTON: &str = "inline-flex min-h-9 items-center justify-center gap-1.5 whitespace-nowrap rounded-md border border-control-border bg-white px-[15px] py-[7px] text-[13px] font-medium leading-5 text-secondary hover:border-primary-hover hover:text-primary";
const PRIMARY_BUTTON: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const TEXT_LINK: &str = "border-0 bg-transparent p-0 text-xs leading-[22px] whitespace-nowrap text-primary hover:text-primary-hover";
const NAV_ITEM: &str = "flex items-center gap-[11px] rounded-md px-4 py-[11px] text-sm text-secondary hover:bg-surface hover:text-primary aria-[current=page]:bg-primary-soft aria-[current=page]:font-semibold aria-[current=page]:text-[#0958d9] max-[900px]:gap-2 max-[900px]:px-[11px] max-[900px]:py-2.5 max-[900px]:text-xs max-[640px]:gap-1 max-[640px]:px-2 max-[640px]:py-[7px] max-[640px]:text-[11px] [&>span]:w-5 [&>span]:text-center [&>span]:text-[19px] [&>span]:font-normal max-[640px]:[&>span]:hidden";
const FIELDS_GRID: &str = "grid grid-cols-2 items-start gap-[18px] max-[640px]:grid-cols-1 [&>div]:content-start [&_input:not([type=checkbox])]:w-full [&_select]:w-full [&_label]:text-[13px] [&_.text-xs]:text-[11px] [&_.text-xs]:leading-relaxed [&_.text-xs]:text-muted";
const FIELD_HINT: &str = "mt-[9px] mb-0 text-[11px] leading-[1.7] text-muted";

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
                <div class="grid min-h-screen grid-cols-[216px_minmax(0,1fr)] max-[1180px]:grid-cols-[186px_minmax(0,1fr)] max-[900px]:grid-cols-[165px_minmax(0,1fr)] max-[640px]:block">
                    <aside class="sticky top-0 flex h-screen flex-col border-r border-border bg-white px-3 max-[900px]:px-[9px] max-[640px]:static max-[640px]:h-auto max-[640px]:flex-row max-[640px]:items-center max-[640px]:gap-[15px] max-[640px]:border-r-0 max-[640px]:border-b max-[640px]:px-4">
                        <a class="flex h-[72px] items-center gap-2.5 px-3 text-xl tracking-[-.6px] max-[900px]:gap-2 max-[900px]:px-2 max-[900px]:text-[17px] max-[640px]:h-[60px] max-[640px]:shrink-0 max-[640px]:px-0 max-[640px]:text-base" href="/"><span class="grid size-[31px] place-items-center rounded-lg bg-primary text-[22px] font-bold text-white shadow-sm max-[900px]:size-7 max-[900px]:text-[19px] max-[640px]:size-[27px] max-[640px]:rounded-md max-[640px]:text-lg" aria-hidden="true">"L"</span><strong>"LLMProxy"</strong></a>
                        <div class="px-4 pt-7 pb-3 text-[11px] tracking-[1px] text-muted max-[900px]:px-[11px] max-[900px]:pt-6 max-[640px]:hidden">"工作空间"</div>
                        <nav class="grid gap-1.5 max-[640px]:ml-auto max-[640px]:flex max-[640px]:gap-0.5" aria-label="主导航">
                            <a class=(NAV_ITEM) href="/" aria-current=(if routes_page { None } else { Some("page") })><span aria-hidden="true">"◈"</span>"Provider 管理"</a>
                            <a class=(NAV_ITEM) href="/routes" aria-current=(if routes_page { Some("page") } else { None })><span aria-hidden="true">"⇄"</span>"路由概览"</a>
                        </nav>
                        <div class="mt-auto border-t border-border px-3.5 pt-5 pb-[25px] text-xs text-secondary max-[900px]:px-[7px] max-[900px]:py-5 max-[900px]:text-[10px] max-[640px]:hidden [&_small]:mt-2.5 [&_small]:ml-[13px] [&_small]:block [&_small]:text-[11px] [&_small]:text-muted max-[900px]:[&_small]:text-[9px]"><span class="mr-[7px] inline-block size-1.5 rounded-full bg-[#52c41a]"></span>"本地开发环境"<small>"连接与凭据，统一管理"</small></div>
                    </aside>
                    <div class="flex min-w-0 flex-col">
                        <header class="h-16 border-b border-border bg-white text-[13px] text-muted max-[640px]:hidden"><div class="mx-auto flex size-full max-w-[1480px] items-center justify-between px-9 max-[1180px]:px-6 [&_strong]:font-medium [&_strong]:text-secondary"><span>"控制台"<span class="mx-[13px] text-[#c8cdd5]">"/"</span><strong>(page_title)</strong></span><span class="rounded border border-border bg-[#f7f9fb] px-[9px] py-[5px] text-[10px] tracking-[1.1px] text-muted">"LOCAL"</span></div></header>
                        <main id="main" class="mx-auto w-full max-w-[1480px] flex-1 px-9 pt-[34px] pb-8 min-[1600px]:pt-[42px] max-[1180px]:px-6 max-[1180px]:py-7 max-[640px]:px-4 max-[640px]:pt-[25px] max-[640px]:pb-6">(slot)</main>
                        <footer class="flex items-center gap-3.5 border-t border-border/30 px-9 py-[22px] text-[10px] tracking-[.1px] text-muted max-[640px]:gap-2.5 max-[640px]:px-4 max-[640px]:py-[18px] max-[640px]:text-[9px] [&>span]:border-l [&>span]:border-border [&>span]:pl-3.5 max-[640px]:[&>span]:pl-2.5">"LLMProxy"<span>"一个入口，连接你的模型服务"</span></footer>
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
            <div><div class="mb-[9px] text-xs text-muted max-[640px]:text-[11px]">"模型服务"</div><h1>"路由概览"</h1><p>"查看各协议入口与当前使用的 Provider。"</p></div>
            <a class=(BUTTON) href="/">"管理 Provider"</a>
        </section>
        <section class="grid grid-cols-3 gap-[18px] max-[1180px]:gap-3 max-[900px]:grid-cols-1 max-[640px]:gap-2.5" id="routes" aria-label="三个协议的当前 Provider">
            for protocol in PROTOCOLS {
                <a class="min-w-0 rounded-lg border border-border bg-white px-[22px] pt-5 pb-[18px] shadow-xs hover:border-[#91caff] max-[1180px]:px-[17px] max-[1180px]:py-[18px] max-[900px]:grid max-[900px]:grid-cols-2 max-[900px]:items-center max-[900px]:gap-x-4 max-[900px]:gap-y-[7px] max-[900px]:px-[19px] max-[900px]:py-[17px] max-[640px]:px-4 max-[640px]:py-[15px]" href=(format!("/?protocol={}", protocol.as_str()))>
                    <div class="flex min-w-0 items-center gap-2.5 max-[900px]:col-start-1 max-[900px]:row-start-1"><span class="grid size-[29px] shrink-0 place-items-center rounded-md border border-[#e3edff] bg-[#f4f8ff] text-[13px] font-semibold text-primary max-[640px]:size-6 max-[640px]:text-[11px]" aria-hidden="true">(match protocol { Protocol::OpenAiChat => "C", Protocol::OpenAiResponses => "R", Protocol::AnthropicMessages => "A" })</span><span class="text-xs font-semibold text-secondary max-[1180px]:text-[11px] max-[900px]:text-xs max-[640px]:text-[11px]">(protocol_label(protocol))</span><span class="ml-auto text-[15px] text-[#bbc4d0] max-[900px]:hidden" aria-hidden="true">"↗"</span></div>
                    if let Some(provider) = all.iter().find(|provider| provider.protocol == protocol && provider.active) {
                        <strong class="mt-[18px] mb-2 block truncate text-lg font-semibold leading-[1.4] max-[900px]:col-start-2 max-[900px]:row-start-1 max-[900px]:m-0 max-[900px]:text-right max-[900px]:text-[15px] max-[640px]:text-[13px]">(provider.name.as_str())</strong><span class="block text-[11px] leading-[18px] text-muted max-[900px]:col-start-2 max-[900px]:row-start-2 max-[900px]:text-right max-[640px]:text-[10px]"><span class="mr-[7px] inline-block size-1.5 rounded-full bg-[#52c41a]"></span>"当前 Provider"</span>
                    } else {
                        <strong class="mt-[18px] mb-2 block truncate text-lg font-semibold leading-[1.4] max-[900px]:col-start-2 max-[900px]:row-start-1 max-[900px]:m-0 max-[900px]:text-right max-[900px]:text-[15px] max-[640px]:text-[13px] font-normal! text-muted">"尚未分配"</strong><span class="block text-[11px] leading-[18px] text-muted max-[900px]:col-start-2 max-[900px]:row-start-2 max-[900px]:text-right max-[640px]:text-[10px]">"启用 Provider 后设为当前服务"</span>
                    }
                    <code class="mt-[18px] block truncate border-t border-border pt-[13px] font-mono text-[11px] text-muted max-[900px]:col-start-1 max-[900px]:m-0 max-[900px]:border-0 max-[900px]:p-0 max-[900px]:text-[10px] max-[640px]:text-[9px]">(protocol.upstream_path())</code>
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
            <div><div class="mb-[9px] text-xs text-muted max-[640px]:text-[11px]">"模型服务"</div><h1>"Provider 管理"</h1><p>"管理上游连接与凭据，为每种协议选择当前服务。"</p></div>
            <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())><span aria-hidden="true">"＋"</span>"新建 Provider"</button>
        </section>
        <section class="providers-panel overflow-visible rounded-lg border border-border bg-white shadow-xs" aria-labelledby="providers-heading">
            <div class="flex items-center justify-between gap-4 px-[22px] pt-[22px] max-[640px]:px-4 max-[640px]:pt-[18px] [&_h2]:m-0 [&_h2]:flex [&_h2]:items-center [&_h2]:gap-[9px] [&_h2]:text-[15px] [&_h2]:font-semibold max-[640px]:[&_h2]:text-sm"><h2 id="providers-heading">"Provider 列表"<span class="grid h-5 min-w-[21px] place-items-center rounded bg-[#f1f3f6] px-[5px] text-[11px] font-normal text-muted">(total)</span></h2><span class="text-xs text-muted">(enabled)" 个已启用"</span></div>
            <form class="flex items-center gap-2.5 px-[22px] py-5 max-[1180px]:flex-wrap max-[640px]:gap-2 max-[640px]:p-4 [&_select]:h-9 [&_select]:min-w-[126px] [&_select]:text-xs max-[640px]:[&_select]:w-[100px] max-[640px]:[&_select]:min-w-0 max-[640px]:[&_select]:flex-1 max-[640px]:[&_select]:pr-4 max-[640px]:[&_select]:pl-2 max-[640px]:[&_select]:text-[11px]" method="get" action="/" role="search">
                <div class="flex h-9 w-[280px] min-w-[180px] items-center rounded-md border border-control-border focus-within:border-primary-hover focus-within:ring-2 focus-within:ring-primary/10 max-[1180px]:min-w-[200px] max-[1180px]:flex-1 max-[640px]:h-[35px] max-[640px]:w-full max-[640px]:min-w-full [&>span]:pl-2.5 [&>span]:text-[22px] [&>span]:leading-none [&>span]:text-muted [&_input]:h-[34px] [&_input]:w-full [&_input]:border-0 [&_input]:bg-transparent [&_input]:pl-2 [&_input]:text-xs [&_input]:shadow-none"><span aria-hidden="true">"⌕"</span><input aria-label="搜索名称或主机" name="q" value=(query.q.as_str()) placeholder="搜索名称或主机地址"></div>
                <select name="protocol" aria-label="筛选协议"><option value="">"全部协议"</option>for protocol in PROTOCOLS { <option value=(protocol.as_str()) selected=(query.protocol == protocol.as_str())>(protocol_label(protocol))</option> }</select>
                <select name="state" aria-label="筛选状态"><option value="">"全部状态"</option><option value="enabled" selected=(query.state == "enabled")>"已启用"</option><option value="disabled" selected=(query.state == "disabled")>"已停用"</option><option value="active" selected=(query.state == "active")>"当前使用"</option></select>
                <button class=(BUTTON) type="submit">"查询"</button>
                if !query.q.is_empty() || !query.protocol.is_empty() || !query.state.is_empty() { <a class=(class!(TEXT_LINK, "px-1 max-[640px]:w-full max-[640px]:text-right max-[640px]:text-[11px]")) href="/">"重置"</a> }
            </form>
            if providers.is_empty() {
                <div class="border-t border-border px-6 pt-12 pb-14 text-center max-[640px]:px-5 max-[640px]:pt-[35px] max-[640px]:pb-10 [&_h3]:mt-0 [&_h3]:mb-2.5 [&_h3]:text-base [&_h3]:font-medium [&_h3]:text-secondary max-[640px]:[&_h3]:text-sm [&_p]:mt-0 [&_p]:mb-[23px] [&_p]:text-xs [&_p]:text-muted max-[640px]:[&_p]:text-[11px] max-[640px]:[&_p]:leading-[1.7]"><span class="mb-[17px] block text-4xl font-light text-[#b8c9e3]" aria-hidden="true">"◇"</span><h3>(if all.is_empty() { "连接第一个模型服务" } else { "没有找到匹配的 Provider" })</h3><p>(if all.is_empty() { "添加上游地址与 API Key，即可开始管理你的模型连接。" } else { "尝试调整搜索关键词，或清除筛选条件。" })</p>if all.is_empty() { <button class=(class!(BUTTON, PRIMARY_BUTTON)) type="button" (create.clone())>"新建 Provider"</button> } else { <a class=(class!(BUTTON, PRIMARY_BUTTON)) href="/">"清除筛选"</a> }</div>
            } else {
                data_table(label: "Provider 列表", attrs: attributes! { class="min-w-[760px] [&_th]:text-xs! [&_td]:py-[18px]!" },
                    <thead><tr><th>"名称 / 协议"</th><th>"上游地址"</th><th>"状态"</th><th>"凭据"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        for provider in &providers {
                            <tr id=(format!("provider-{}", provider.id))>
                                <td><button class="block border-0 bg-transparent p-0 text-left text-[13px] font-semibold leading-5 text-[#3b4655] hover:text-primary" type="button" (editor_trigger(cx, &editor, provider.clone().into()))>(provider.name.as_str())</button><span class="mt-[5px] block text-[11px] leading-[18px] text-muted">(protocol_label(provider.protocol))</span></td>
                                <td><span class="whitespace-nowrap text-xs text-secondary">(provider_url(provider))</span><span class="mt-[5px] block text-[11px] leading-[18px] text-muted">"读取超时 "(provider.read_timeout_ms / 1000)" 秒"</span></td>
                                <td><div class="flex max-w-[155px] flex-wrap gap-[5px]">tag(tone: if provider.enabled { TagTone::Success } else { TagTone::Default }, (if provider.enabled { "已启用" } else { "已停用" })) if provider.active { tag(tone: TagTone::Processing, "当前使用") }</div></td>
                                <td><span class="whitespace-nowrap text-[11px] text-muted">(if provider.key_configured { "● 已配置" } else { "未配置" })</span></td>
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
                <div class="flex justify-between gap-4 border-t border-border px-[22px] py-[17px] text-[11px] text-muted max-[640px]:px-4 max-[640px]:py-[15px] max-[900px]:[&>span]:hidden">"显示 "(providers.len())" / "(total)" 个 Provider"<span>"每种协议可选择一个当前服务"</span></div>
            }
        </section>
        <p class="mx-[3px] mt-[17px] mb-0 text-[11px] leading-[1.8] text-muted max-[640px]:text-[10px] [&>span]:mr-[7px] [&>span]:text-[13px]"><span aria-hidden="true">"ⓘ"</span>"已启用表示可供选择；设为当前服务后，该协议的新请求会使用此 Provider。"</p>
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
        <button class=(class!(TEXT_LINK, "text-[#ef6466]! hover:text-[#ff4d4f]!" if delete)) type="button" (trigger) :disabled=$(busy.get())>(label)</button>
        popconfirm(id: id.as_str(), title: title.as_str(), language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="[&_footer]:items-center [&_footer_.gr-button]:h-8! [&_footer_.gr-button]:w-[88px]! [&_footer_.gr-button]:px-3! [&_footer_.gr-button]:py-1! [&_footer_.gr-button]:text-[13px]! [&_footer_.gr-button]:leading-[22px]!" },
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
            attrs: attributes! { cx => class="w-[min(720px,calc(100%_-_32px))]! max-[640px]:w-[calc(100%_-_24px)]! max-[640px]:max-h-[calc(100dvh_-_24px)]! [&_.gr-dialog-header]:px-6 [&_.gr-dialog-header]:pt-5 [&_.gr-dialog-header]:pb-4 [&_h2]:m-0 [&_h2]:text-lg max-[640px]:[&_.gr-dialog-header]:px-[18px] max-[640px]:[&_.gr-dialog-header]:py-4" @close=$(|_event: Event| api_key.set("".to_owned())) },
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
                <div class="min-h-0 overflow-y-auto overscroll-contain px-6 py-[22px] max-[640px]:p-[18px]">
                    <div class="mb-5 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-[13px] text-[13px] leading-[1.7] text-[#cf1322] [&_strong]:mb-1 [&_strong]:block [&_strong]:font-semibold [&_small]:mt-1.5 [&_small]:block [&_small]:text-[11px] [&_small]:text-[#bf5a5b]" role="alert" :hidden=$(error.get().is_empty())><strong>"保存失败"</strong><span>$(error.get())</span><small>"API Key 不会回显；如需新增或更换凭据，请重新输入。"</small></div>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-[22px] [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"基本信息"</h3>
                        <div class=(FIELDS_GRID)>
                            form_field(config: FormFieldConfig::new("name", "Provider 名称").required(), <input id="name" name="name" :value=$(name.get()) @input=$(|event: Event| name.set(event.target.value)) placeholder="例如：OpenAI · Production" maxlength="80" required="" autofocus="">)
                            form_field(config: FormFieldConfig::new("protocol", "接口协议").required(), <select id="protocol" name="protocol" :value=$(protocol.get()) @change=$(|event: Event| protocol.set(event.target.value))>for choice in PROTOCOLS { <option value=(choice.as_str())>(protocol_label(choice))</option> }</select>)
                        </div>
                        <label class="mt-[18px] flex cursor-pointer items-start gap-[9px] text-xs leading-5 text-secondary [&_small]:ml-3 [&_small]:inline [&_small]:text-[11px] [&_small]:text-muted max-[640px]:[&_small]:ml-0 max-[640px]:[&_small]:block"><input type="checkbox" name="enabled" value="true" :checked=$(enabled.get()) @change=$(|event: Event| enabled.set(event.target.checked))><span>"启用此 Provider"<small>"保存后可在列表中设为当前服务。"</small></span></label>
                    </section>
                    <section class="[&+section]:mt-6 [&+section]:border-t [&+section]:border-border [&+section]:pt-[22px] [&_h3]:mt-0 [&_h3]:mb-4 [&_h3]:text-sm [&_h3]:font-semibold">
                        <h3>"连接与凭据"</h3>
                        <div class=(FIELDS_GRID)>
                            <div class="col-span-full">form_field(config: FormFieldConfig::new("upstream-url", "上游地址").required(), <input id="upstream-url" name="upstream_url" type="url" :value=$(upstream_url.get()) @input=$(|event: Event| upstream_url.set(event.target.value)) placeholder="https://api.deepseek.com" required="" aria-describedby="connection-help">)</div>
                        </div>
                        <p class=(FIELD_HINT) id="connection-help">"例如 https://api.deepseek.com；本地服务可填写 http://127.0.0.1:11434。"</p>
                        <div class=(class!(FIELDS_GRID, "mt-[18px]"))>
                            <div class="col-span-full">form_field(config: FormFieldConfig::new("api-key", "API Key"),
                                <input id="api-key" name="api_key" type="password" autocomplete="new-password" :value=$(if open.get() { api_key.get() } else { "".to_owned() }) @input=$(|event: Event| api_key.set(event.target.value)) :placeholder=$(if id.get().is_empty() { "输入 Provider API Key" } else { "留空以保留现有 API Key" }) :required=$(id.get().is_empty()) aria-describedby="api-key-help">
                                <p class=(FIELD_HINT) id="api-key-help">$(if id.get().is_empty() { "必填，凭据加密保存且不会回显。" } else { "留空保留现有凭据，输入新值即可更换。" })</p>
                            )</div>
                            <div class="col-span-full" :hidden=$(protocol.get() != "anthropic_messages")>form_field(config: FormFieldConfig::new("anthropic-version", "Anthropic API 版本").with_hint("请求上游时使用的 anthropic-version。"), <input id="anthropic-version" name="anthropic_version" :value=$(anthropic_version.get()) @input=$(|event: Event| anthropic_version.set(event.target.value)) placeholder="2023-06-01" :disabled=$(protocol.get() != "anthropic_messages") aria-describedby="anthropic-version-help">)</div>
                        </div>
                    </section>
                    <details class="group mt-6 rounded-md border border-border" :open=$(advanced.get())>
                        <summary class="flex cursor-pointer list-none items-center gap-3 px-3.5 py-[13px] text-[13px] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#91caff] [&::-webkit-details-marker]:hidden max-[640px]:flex-wrap max-[640px]:gap-1.5" @click=$(|event: Event| { event.prevent_default(); advanced.toggle(); })><span>"超时设置"</span><span class="ml-auto text-[10px] text-muted max-[640px]:order-2 max-[640px]:w-full">"连接 "$(connect_timeout.get())" / 读取 "$(read_timeout.get())" / 写入 "$(write_timeout.get())" ms"</span><span class="text-muted transition-transform duration-150 group-open:rotate-180 max-[640px]:ml-auto" aria-hidden="true">"⌄"</span></summary>
                        <div class="px-3.5 pb-4 [&>p]:mt-0 [&>p]:mb-3.5"><p class=(FIELD_HINT)>"单位为毫秒。读取超时表示等待上游数据的最长间隔。"</p><div class=(class!(FIELDS_GRID, "grid-cols-3! gap-3.5! max-[640px]:grid-cols-1!"))>
                            form_field(config: FormFieldConfig::new("connect-timeout", "连接超时").required(), <input id="connect-timeout" name="connect_timeout_ms" type="number" min="1" :value=$(connect_timeout.get()) @input=$(|event: Event| connect_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                            form_field(config: FormFieldConfig::new("read-timeout", "读取超时").required(), <input id="read-timeout" name="read_timeout_ms" type="number" min="1" :value=$(read_timeout.get()) @input=$(|event: Event| read_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                            form_field(config: FormFieldConfig::new("write-timeout", "写入超时").required(), <input id="write-timeout" name="write_timeout_ms" type="number" min="1" :value=$(write_timeout.get()) @input=$(|event: Event| write_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                        </div></div>
                    </details>
                </div>
                <footer class="flex shrink-0 justify-end gap-2.5 border-t border-border bg-[#fafbfc] px-6 py-[15px] max-[640px]:px-[18px] max-[640px]:py-4"><button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button><button class=(class!(BUTTON, PRIMARY_BUTTON)) type="submit" :disabled=$(busy.get())>$(if id.get().is_empty() { "创建 Provider" } else { "保存修改" })</button></footer>
            </form>
        )
    })
}
