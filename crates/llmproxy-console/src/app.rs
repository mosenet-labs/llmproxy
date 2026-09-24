use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ProviderInput, ProviderStore, ProviderView};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body, Next, Slot,
        content::Form,
        error::{forbidden, see_other},
        layer, layout, page,
        request::headers,
        response::Response,
        route,
    },
    runtime::{Event, Signal, signal},
    view::{Attributes, View, attributes, component, view},
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
pub async fn shell(slot: Slot<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="zh-CN">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <meta name="color-scheme" content="light">
                <title>"Provider 管理 · LLMProxy"</title>
                head_assets()
                <link rel="stylesheet" href="/assets/console.css">
                topcoat::runtime::script()
            </head>
            <body>
                <div class="console-shell">
                    <aside class="sidebar">
                        <a class="brand" href="/"><span class="brand-symbol" aria-hidden="true">"L"</span><strong>"LLMProxy"</strong></a>
                        <div class="sidebar-caption">"工作空间"</div>
                        <nav aria-label="主导航">
                            <a class="nav-item nav-active" href="/" aria-current="page"><span aria-hidden="true">"◈"</span>"Provider 管理"</a>
                            <a class="nav-item" href="/#routes"><span aria-hidden="true">"⇄"</span>"路由概览"</a>
                        </nav>
                        <div class="sidebar-footer"><span class="local-dot"></span>"本地开发环境"<small>"连接与凭据，统一管理"</small></div>
                    </aside>
                    <div class="workspace">
                        <header class="topbar"><span>"控制台"<span class="breadcrumb-divider">"/"</span><strong>"Provider 管理"</strong></span><span class="environment-label">"LOCAL"</span></header>
                        <main id="main">(slot)</main>
                        <footer class="workspace-footer">"LLMProxy"<span>"一个入口，连接你的模型服务"</span></footer>
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
    #[serde(default)]
    notice: String,
    #[serde(default)]
    provider_name: String,
}

fn success_location(action: &str, name: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("notice", action)
        .append_pair("provider_name", name)
        .finish();
    format!("/?{query}")
}

fn success_message(query: &ListQuery) -> String {
    let action = match query.notice.as_str() {
        "created" => "已创建",
        "updated" => "已保存",
        "enable" => "已启用",
        "disable" => "已停用",
        "activate" => "已设为当前服务",
        "delete" => "已删除",
        _ => return String::new(),
    };
    if query.provider_name.is_empty() {
        return String::new();
    }
    format!("「{}」{action}", query.provider_name)
}

#[page("/")]
pub async fn list(cx: &Cx, Form(query): Form<ListQuery>) -> Result<impl View> {
    let all = app_context::<AppState>(cx).store.list().await?;
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
    Ok(view! { provider_list(all: &all, providers: &filtered, query: &query, error: None) })
}

#[component]
async fn provider_list(
    cx: &Cx,
    all: &[ProviderView],
    providers: &[ProviderView],
    query: &ListQuery,
    error: Option<&str>,
    #[default] editor_input: Option<&ProviderForm>,
    #[default] editor_error: Option<&str>,
    #[default] editor_open: bool,
) -> Result<impl View> {
    let defaults = ProviderForm::default();
    let editor = EditorSignals::new(
        cx,
        editor_input.unwrap_or(&defaults),
        editor_error,
        editor_open,
    );
    let create = editor_trigger(cx, &editor, ProviderForm::default());
    let csrf = &app_context::<AppState>(cx).csrf;
    let total = all.len();
    let enabled = all.iter().filter(|provider| provider.enabled).count();
    let notice = signal(cx, || match error {
        Some(error) => error.to_owned(),
        None => success_message(query),
    });
    Ok(view! {
        provider_editor(editor: &editor)
        notification(
            message: &notice,
            title: if error.is_some() { "操作失败" } else { "操作成功" },
            tone: if error.is_some() { NotificationTone::Error } else { NotificationTone::Success },
            language: UiLanguage::ChineseSimplified,
        )
        <section class="page-heading">
            <div><div class="eyebrow">"模型服务"</div><h1>"Provider 管理"</h1><p>"管理上游连接与凭据，为每种协议选择当前服务。"</p></div>
            <button class="button button-primary" type="button" (create.clone())><span aria-hidden="true">"＋"</span>"新建 Provider"</button>
        </section>
        <section class="route-overview" id="routes" aria-label="三个协议的当前 Provider">
            for protocol in PROTOCOLS {
                <a class="route-card" href=(format!("/?protocol={}", protocol.as_str()))>
                    <div class="route-card-top"><span class="route-icon" aria-hidden="true">(match protocol { Protocol::OpenAiChat => "C", Protocol::OpenAiResponses => "R", Protocol::AnthropicMessages => "A" })</span><span class="route-label">(protocol_label(protocol))</span><span class="route-arrow" aria-hidden="true">"↗"</span></div>
                    if let Some(provider) = all.iter().find(|provider| provider.protocol == protocol && provider.active) {
                        <strong class="route-provider">(provider.name.as_str())</strong><span class="route-detail"><span class="status-dot"></span>"当前 Provider"</span>
                    } else {
                        <strong class="route-provider route-unbound">"尚未分配"</strong><span class="route-detail">"启用 Provider 后设为当前服务"</span>
                    }
                    <code>(protocol.upstream_path())</code>
                </a>
            }
        </section>
        <section class="panel providers-panel" aria-labelledby="providers-heading">
            <div class="panel-heading"><h2 id="providers-heading">"Provider 列表"<span class="count-badge">(total)</span></h2><span class="panel-summary">(enabled)" 个已启用"</span></div>
            <form class="filter-bar" method="get" action="/" role="search">
                <div class="search-field"><span aria-hidden="true">"⌕"</span><input aria-label="搜索名称或主机" name="q" value=(query.q.as_str()) placeholder="搜索名称或主机地址"></div>
                <select name="protocol" aria-label="筛选协议"><option value="">"全部协议"</option>for protocol in PROTOCOLS { <option value=(protocol.as_str()) selected=(query.protocol == protocol.as_str())>(protocol_label(protocol))</option> }</select>
                <select name="state" aria-label="筛选状态"><option value="">"全部状态"</option><option value="enabled" selected=(query.state == "enabled")>"已启用"</option><option value="disabled" selected=(query.state == "disabled")>"已停用"</option><option value="active" selected=(query.state == "active")>"当前使用"</option></select>
                <button class="button" type="submit">"查询"</button>
                if !query.q.is_empty() || !query.protocol.is_empty() || !query.state.is_empty() { <a class="text-link reset-link" href="/">"重置"</a> }
            </form>
            if providers.is_empty() {
                <div class="empty-state"><span class="empty-icon" aria-hidden="true">"◇"</span><h3>(if all.is_empty() { "连接第一个模型服务" } else { "没有找到匹配的 Provider" })</h3><p>(if all.is_empty() { "添加上游地址与 API Key，即可开始管理你的模型连接。" } else { "尝试调整搜索关键词，或清除筛选条件。" })</p>if all.is_empty() { <button class="button button-primary" type="button" (create.clone())>"新建 Provider"</button> } else { <a class="button button-primary" href="/">"清除筛选"</a> }</div>
            } else {
                data_table(label: "Provider 列表",
                    <thead><tr><th>"名称 / 协议"</th><th>"上游地址"</th><th>"状态"</th><th>"凭据"</th><th class="actions-heading">"操作"</th></tr></thead>
                    <tbody>
                        for provider in providers {
                            <tr>
                                <td><button class="provider-name" type="button" (editor_trigger(cx, &editor, provider.clone().into()))>(provider.name.as_str())</button><span class="table-secondary">(protocol_label(provider.protocol))</span></td>
                                <td><span class="endpoint">(provider_url(provider))</span><span class="table-secondary">"读取超时 "(provider.read_timeout_ms / 1000)" 秒"</span></td>
                                <td><div class="status-tags">tag(tone: if provider.enabled { TagTone::Success } else { TagTone::Default }, (if provider.enabled { "已启用" } else { "已停用" })) if provider.active { tag(tone: TagTone::Processing, "当前使用") }</div></td>
                                <td><span class="credential-status">(if provider.key_configured { "● 已配置" } else { "未配置" })</span></td>
                                <td><div class="row-actions">
                                    <button class="text-link" type="button" (editor_trigger(cx, &editor, provider.clone().into()))>"编辑"</button>
                                    if provider.enabled && !provider.active {
                                        action_form(csrf: csrf, provider: provider, action: "activate", label: "设为当前服务")
                                    }
                                    if provider.enabled {
                                        provider_action_confirmation(provider: provider, csrf: csrf, delete: false)
                                    } else {
                                        action_form(csrf: csrf, provider: provider, action: "enable", label: "启用")
                                        provider_action_confirmation(provider: provider, csrf: csrf, delete: true)
                                    }
                                </div></td>
                            </tr>
                        }
                    </tbody>
                )
                <div class="table-footer">"显示 "(providers.len())" / "(total)" 个 Provider"<span>"每种协议可选择一个当前服务"</span></div>
            }
        </section>
        <p class="usage-note"><span aria-hidden="true">"ⓘ"</span>"已启用表示可供选择；设为当前服务后，该协议的新请求会使用此 Provider。"</p>
    })
}

#[component]
async fn provider_action_confirmation(
    cx: &Cx,
    provider: &ProviderView,
    csrf: &str,
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
    Ok(view! {
        <button class=(if delete { "text-link danger-link" } else { "text-link" }) type="button" (trigger)>(label)</button>
        popconfirm(id: id.as_str(), title: title.as_str(), language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="provider-action-confirmation" },
            <form class="inline-form" action="/providers/action" method="post"><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><button class="gr-button gr-button-danger" type="submit">(format!("确认{label}"))</button></form>
        )
    })
}

#[component]
async fn action_form(
    csrf: &str,
    provider: &ProviderView,
    action: &str,
    label: &str,
) -> Result<impl View> {
    Ok(
        view! { <form class="inline-form" action="/providers/action" method="post"><input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="id" value=(provider.id)><input type="hidden" name="version" value=(provider.version)><input type="hidden" name="action" value=(action)><button class="text-link" type="submit">(label)</button></form> },
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
    let input = match query.id {
        Some(id) => app_context::<AppState>(cx).store.get(id).await?.into(),
        None => ProviderForm::default(),
    };
    Ok(view! { editor_page(input: &input, error: None) })
}

#[page(POST "/providers/save")]
pub async fn save(cx: &Cx, Form(mut input): Form<ProviderForm>) -> Result<impl View> {
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
    match result {
        Ok(name) => Err(see_other(success_location(
            if input.id.is_some() {
                "updated"
            } else {
                "created"
            },
            &name,
        ))
        .into()),
        Err(error) => Ok(view! { editor_page(input: &input, error: Some(error.as_str())) }),
    }
}

#[derive(Deserialize)]
pub struct ActionForm {
    #[serde(default)]
    csrf: String,
    id: i64,
    version: u64,
    action: String,
}

#[page(POST "/providers/action")]
pub async fn perform_action(cx: &Cx, Form(input): Form<ActionForm>) -> Result<impl View> {
    check_csrf(cx, &input.csrf)?;
    let store = &app_context::<AppState>(cx).store;
    let label = match input.action.as_str() {
        "activate" => "设为当前服务",
        "enable" => "启用",
        "disable" => "停用",
        "delete" => "删除",
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
        _ => return Err(topcoat::router::error::bad_request("无效的操作").into()),
    };
    if result.is_ok() {
        return Err(see_other(success_location(&input.action, &name)).into());
    }
    let error = format!("「{name}」{label}失败：{}", result.unwrap_err());
    let all = store.list().await?;
    let providers = all.clone();
    let query = ListQuery::default();
    Ok(
        view! { provider_list(all: &all, providers: &providers, query: &query, error: Some(error.as_str())) },
    )
}

#[component]
async fn editor_page(cx: &Cx, input: &ProviderForm, error: Option<&str>) -> Result<impl View> {
    let all = app_context::<AppState>(cx).store.list().await?;
    let query = ListQuery::default();
    Ok(view! {
        provider_list(all: &all, providers: &all, query: &query, error: None, editor_input: Some(input), editor_error: error, editor_open: true)
    })
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
async fn provider_editor(cx: &Cx, editor: &EditorSignals) -> Result<impl View> {
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
    Ok(view! {
        dialog(config: DialogConfig::new("provider-dialog", "Provider 配置"),
            open: Some(open), title: Some(title), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { cx => class="provider-dialog" @close=$(|_event: Event| api_key.set("".to_owned())) },
            <form class="provider-form" action="/providers/save" method="post" autocomplete="off"
                @submit=$(|event: Event| { if busy.get() { event.prevent_default(); } else { busy.set(true); } })>
                <input type="hidden" name="csrf" value=(csrf.as_str())>
                <input type="hidden" name="id" :value=$(id.get()) :disabled=$(id.get().is_empty())>
                <input type="hidden" name="version" :value=$(version.get()) :disabled=$(id.get().is_empty())>
                <div class="provider-form-body">
                    <div class="alert alert-error" role="alert" :hidden=$(error.get().is_empty())><strong>"保存失败"</strong><span>$(error.get())</span><small>"API Key 不会回显；如需新增或更换凭据，请重新输入。"</small></div>
                    <section class="dialog-section">
                        <h3>"基本信息"</h3>
                        <div class="fields-grid">
                            form_field(config: FormFieldConfig::new("name", "Provider 名称").required(), <input id="name" name="name" :value=$(name.get()) @input=$(|event: Event| name.set(event.target.value)) placeholder="例如：OpenAI · Production" maxlength="80" required="" autofocus="">)
                            form_field(config: FormFieldConfig::new("protocol", "接口协议").required(), <select id="protocol" name="protocol" :value=$(protocol.get()) @change=$(|event: Event| protocol.set(event.target.value))>for choice in PROTOCOLS { <option value=(choice.as_str())>(protocol_label(choice))</option> }</select>)
                        </div>
                        <label class="checkbox-label provider-enabled"><input type="checkbox" name="enabled" value="true" :checked=$(enabled.get()) @change=$(|event: Event| enabled.set(event.target.checked))><span>"启用此 Provider"<small>"保存后可在列表中设为当前服务。"</small></span></label>
                    </section>
                    <section class="dialog-section">
                        <h3>"连接与凭据"</h3>
                        <div class="fields-grid">
                            <div class="full-field">form_field(config: FormFieldConfig::new("upstream-url", "上游地址").required(), <input id="upstream-url" name="upstream_url" type="url" :value=$(upstream_url.get()) @input=$(|event: Event| upstream_url.set(event.target.value)) placeholder="https://api.deepseek.com" required="" aria-describedby="connection-help">)</div>
                        </div>
                        <p class="field-hint" id="connection-help">"例如 https://api.deepseek.com；本地服务可填写 http://127.0.0.1:11434。"</p>
                        <div class="fields-grid credentials-grid">
                            <div class="full-field">form_field(config: FormFieldConfig::new("api-key", "API Key"),
                                <input id="api-key" name="api_key" type="password" autocomplete="new-password" :value=$(if open.get() { api_key.get() } else { "".to_owned() }) @input=$(|event: Event| api_key.set(event.target.value)) :placeholder=$(if id.get().is_empty() { "输入 Provider API Key" } else { "留空以保留现有 API Key" }) :required=$(id.get().is_empty()) aria-describedby="api-key-help">
                                <p class="field-hint" id="api-key-help">$(if id.get().is_empty() { "必填，凭据加密保存且不会回显。" } else { "留空保留现有凭据，输入新值即可更换。" })</p>
                            )</div>
                            <div class="full-field anthropic-field" :hidden=$(protocol.get() != "anthropic_messages")>form_field(config: FormFieldConfig::new("anthropic-version", "Anthropic API 版本").with_hint("请求上游时使用的 anthropic-version。"), <input id="anthropic-version" name="anthropic_version" :value=$(anthropic_version.get()) @input=$(|event: Event| anthropic_version.set(event.target.value)) placeholder="2023-06-01" :disabled=$(protocol.get() != "anthropic_messages") aria-describedby="anthropic-version-help">)</div>
                        </div>
                    </section>
                    <details class="timeout-settings" :open=$(advanced.get())>
                        <summary @click=$(|event: Event| { event.prevent_default(); advanced.toggle(); })><span>"超时设置"</span><span class="timeout-summary">"连接 "$(connect_timeout.get())" / 读取 "$(read_timeout.get())" / 写入 "$(write_timeout.get())" ms"</span><span class="details-chevron" aria-hidden="true">"⌄"</span></summary>
                        <div class="timeout-content"><p class="field-hint">"单位为毫秒。读取超时表示等待上游数据的最长间隔。"</p><div class="fields-grid timeout-grid">
                            form_field(config: FormFieldConfig::new("connect-timeout", "连接超时").required(), <input id="connect-timeout" name="connect_timeout_ms" type="number" min="1" :value=$(connect_timeout.get()) @input=$(|event: Event| connect_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                            form_field(config: FormFieldConfig::new("read-timeout", "读取超时").required(), <input id="read-timeout" name="read_timeout_ms" type="number" min="1" :value=$(read_timeout.get()) @input=$(|event: Event| read_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                            form_field(config: FormFieldConfig::new("write-timeout", "写入超时").required(), <input id="write-timeout" name="write_timeout_ms" type="number" min="1" :value=$(write_timeout.get()) @input=$(|event: Event| write_timeout.set(event.target.value)) @invalid=$(|_event: Event| advanced.set(true)) required="">)
                        </div></div>
                    </details>
                </div>
                <footer class="provider-form-footer"><button class="button" type="button" (close) :disabled=$(busy.get())>"取消"</button><button class="button button-primary" type="submit" :disabled=$(busy.get())>$(if id.get().is_empty() { "创建 Provider" } else { "保存修改" })</button></footer>
            </form>
        )
    })
}
