use crate::app::{check_csrf, group_store};
use llmproxy_store::{GroupView, VirtualKeyView};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{content::Form, href, page, response::Response, route},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, class, component, view},
};
use topcoat_ant_design::icons::{AUDIT_OUTLINED, PLUS_OUTLINED, TEAM_OUTLINED};
use topcoat_ant_design::{
    DataTableDensity, NativeDialogConfig, NotificationTone, TagTone, UiLanguage, data_table,
    native_dialog, native_dialog_close_attributes, native_dialog_trigger_attributes, notification,
    tag,
};

mod editor;
pub(super) mod group;
use super::table::{Pagination, pagination};
mod resources;
use editor::{group_editor, key_editor, secret_dialog};
pub(crate) use resources::{remove_group_resource, save_group_resources};

async fn find_group(cx: &Cx, id: i64) -> Result<GroupView> {
    crate::app::store(cx)
        .list_groups()
        .await?
        .into_iter()
        .find(|group| group.id == id)
        .ok_or_else(|| topcoat::router::error::not_found().into())
}

#[component]
async fn group_heading(cx: &Cx, group: &GroupView) -> Result<impl View> {
    let __cx = cx;
    Ok(view! {
        <header class=(super::providers::PAGE_HEADING)>
            <div>
                <h1 class="sr-only">(format!("{}详情", group.name))</h1>
                <p>"所属资源组："<strong class="text-heading" data-current-group=(group.id.to_string())>(group.name.as_str())</strong>
                    if !group.enabled { <span class="ml-2">"（已停用）"</span> }
                </p>
            </div>
            <div class="flex flex-wrap items-center gap-3">
                <a class=(BUTTON) href=(crate::app::scoped_href(cx, href!(groups) .resolve(cx)))>"返回资源组"</a>
            </div>
        </header>
    })
}

const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-solid border-control-border bg-white px-4 text-sm font-medium text-heading hover:border-primary-hover hover:text-primary";
const PRIMARY: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const LINK: &str = "border-0 bg-transparent p-0 text-sm text-primary hover:text-primary-hover";
const DIALOG: &str =
    "whitespace-normal [&_.gr-native-dialog-header]:py-4 [&_h2]:m-0 [&_h2]:text-lg";
const FOOTER: &str = "flex shrink-0 justify-end gap-3 border-t border-border bg-white px-6 py-4";

type Outcome = std::result::Result<String, String>;

struct Controls {
    refresh: Signal<f64>,
    success: Signal<String>,
    failure: Signal<String>,
}

impl Controls {
    fn new(cx: &Cx) -> Self {
        Self {
            refresh: signal(cx, || 0.0),
            success: signal(cx, String::new),
            failure: signal(cx, String::new),
        }
    }
}

#[component]
async fn feedback(cx: &Cx, controls: &Controls) -> Result<impl View> {
    let __cx = cx;
    Ok(view! {
        notification(message: &controls.success, title: "操作成功", tone: NotificationTone::Success, language: UiLanguage::ChineseSimplified)
        notification(message: &controls.failure, title: "操作失败", tone: NotificationTone::Error, language: UiLanguage::ChineseSimplified)
    })
}

#[component]
pub async fn group_selector(cx: &Cx, scope: &str) -> Result<impl View> {
    let store = group_store(cx, scope);
    let group_rows = store.list_groups().await?;
    let csrf = crate::app::auth::csrf_token(cx);
    let selector_id = format!("{scope}-group");
    Ok(view! {
        <form method="post" action="/ui/groups/select" class="m-0 flex min-w-0 flex-wrap items-center gap-2" aria-label="切换资源组" data-group-selector=(scope)>
            <input type="hidden" name="csrf" value=(csrf.as_str())>
            <input type="hidden" name="return_to" value=(format!("/ui/{scope}"))>
            <input type="hidden" name="space" value=(crate::app::store(cx).space_id().unwrap().to_string())>
            <label for=(selector_id.as_str()) class="whitespace-nowrap text-[13px] text-secondary">"资源组"</label>
            <select id=(selector_id.as_str()) name="group_id" class="h-9! w-[160px] max-[640px]:w-[140px]" aria-label="当前资源组">
                #[key(group.id)]
                for group in &group_rows {
                    <option value=(group.id.to_string()) selected=(group.id == store.group_id())>(if group.enabled { group.name.clone() } else { format!("{}（已停用）", group.name) })</option>
                }
            </select>
            <button class=(BUTTON) type="submit">"切换"</button>
        </form>
    })
}

#[page]
pub async fn groups(cx: &Cx) -> Result<impl View> {
    crate::app::request_connection(cx);
    let controls = Controls::new(cx);
    let _revision = controls.refresh.get();
    Ok(view! {
        feedback(controls: &controls)
        group_workspace(controls: &controls)
    })
}

#[component]
async fn group_workspace(cx: &Cx, controls: &Controls) -> Result<impl View> {
    let store = crate::app::store(cx);
    let group_rows = store.list_groups().await?;
    let paging = Pagination::new(cx);
    let page_range = paging.range(group_rows.len());
    let csrf = crate::app::auth::csrf_token(cx);
    let models = store.list_all_models().await?;
    let routes = store.list_all_routes().await?;
    let new_group = native_dialog_trigger_attributes(cx, "group-create");
    Ok(view! {
        <section class="w-full min-w-0">
            <header class=(super::providers::PAGE_HEADING)>
                <div>
                    <h1 class="sr-only">"资源组"</h1>
                    <p>"模型和路由可加入多个组；虚拟 Key 只属于一个组，按组授权调用。"</p>
                </div>
                <button class=(class!(BUTTON, PRIMARY)) type="button" (new_group)>
                    icon(data: PLUS_OUTLINED, size: 14)
                    "新增组"
                </button>
            </header>
            <section class="min-w-0 overflow-hidden rounded-lg border border-solid border-border bg-white" aria-labelledby="group-list-title">
                <header class="px-6 py-5 max-[640px]:px-4">
                    <h2 id="group-list-title" class="m-0 text-base font-semibold">"全部资源组"<span class="ml-2 text-sm font-normal text-muted">(group_rows.len())</span></h2>
                    <p class="mb-0 mt-1 text-[13px] text-secondary">"进入组详情，在 Models 中添加模型或路由，在 Keys 中分配调用凭据。"</p>
                </header>
                data_table(
                    label: "资源组列表",
                    density: DataTableDensity::Compact,
                    attrs: attributes! { class="min-w-[660px] [&_th]:px-6! [&_td]:px-6!" },
                    <thead><tr><th>"组名称"</th><th>"模型"</th><th>"路由"</th><th>"状态"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>
                        #[key(group.id)]
                        for group in &group_rows[page_range] {
                            <tr id=(format!("group-{}", group.id))>
                                <td>
                                    <div class="flex items-center gap-3">
                                        icon(data: TEAM_OUTLINED, size: 18, attrs: attributes! { class="shrink-0 text-muted" aria-hidden="true" })
                                        <strong class="block max-w-[360px] truncate font-medium" title=(group.name.as_str())>(group.name.as_str())</strong>
                                    </div>
                                </td>
                                <td class="tabular-nums">(models.iter().filter(|m| m.group_ids.contains(&group.id)).count())</td>
                                <td class="tabular-nums">(routes.iter().filter(|r| r.group_ids.contains(&group.id)).count())</td>
                                <td>tag(tone: if group.enabled { TagTone::Success } else { TagTone::Default }, (if group.enabled { "已启用" } else { "已停用" }))</td>
                                <td>
                                    <div class="flex justify-end gap-4">
                                        <a class=(LINK) href=(crate::app::scoped_href(cx, href!(group::details, group::GroupId(group.id)) .resolve(cx)))>"详情"</a>
                                        <button class=(LINK) type="button" (native_dialog_trigger_attributes(cx, &format!("group-edit-{}", group.id)))>"编辑"</button>
                                    </div>
                                    group_editor(group: Some(group), controls: controls, csrf: csrf.as_str())
                                </td>
                            </tr>
                        }
                    </tbody>
                )
                pagination(state: &paging, total: group_rows.len(), id: "groups-page-size", label: "资源组列表分页")
            </section>
            <p class="mb-0 mt-4 text-[13px] text-muted">"停用组会立即阻止本组所有 Key 的模型调用；重新启用组后可恢复。"</p>
        </section>
        group_editor(group: None, controls: controls, csrf: csrf.as_str())
    })
}

#[page("/ui/keys")]
pub async fn keys(cx: &Cx) -> Result<impl View> {
    crate::app::request_connection(cx);
    let store = group_store(cx, "keys");
    let group_id = store.group_id();
    let group = find_group(cx, group_id).await?;
    let controls = Controls::new(cx);
    let secret = signal(cx, String::new);
    let created = signal(cx, || false);
    let models = store.list_models().await?;
    let routes = store.list_routes().await?;
    let csrf = crate::app::auth::csrf_token(cx);
    let refresh = controls.refresh.clone();
    let success = controls.success.clone();
    let failure = controls.failure.clone();
    Ok(view! {
        feedback(controls: &controls)
        <header class="mb-5 flex flex-wrap items-center justify-between gap-3">
            <div class="flex flex-wrap items-center gap-3">
                group_selector(scope: "keys")
                <span
                    class="text-[13px] text-secondary"
                    data-current-group=(group_id.to_string())
                >
                    (format!("当前组：{}", group.name))
                </span>
            </div>
            <a class=(BUTTON) href=(crate::app::scoped_href(cx, href!(group::details, group::GroupId(group_id)) .resolve(cx)))>
                "管理组内模型"
            </a>
        </header>
        key_workspace(
            group_id: group_id,
            revision: $(refresh.get()),
            refresh: $(refresh),
            success: $(success),
            failure: $(failure)
        )
        key_editor(
            group_id: group_id,
            models: &models,
            routes: &routes,
            controls: &controls,
            csrf: csrf.as_str(),
            secret: &secret,
            created: &created
        )
        secret_dialog(secret: &secret, open: &created)
    })
}

#[shard("/ui/_topcoat/runtime/shards/key-workspace")]
pub async fn key_workspace(
    cx: &Cx,
    group_id: i64,
    revision: f64,
    refresh: Signal<f64>,
    success: Signal<String>,
    failure: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let controls = Controls {
        refresh,
        success,
        failure,
    };
    let store = crate::app::store(cx).for_group(group_id);
    let current = find_group(cx, group_id).await?;
    let key_rows = store.list_virtual_keys().await?;
    let paging = Pagination::new(cx);
    let page_range = paging.range(key_rows.len());
    let csrf = crate::app::auth::csrf_token(cx);
    let new_key = native_dialog_trigger_attributes(cx, "key-create");
    Ok(view! {
        <section class="w-full min-w-0">
            if !current.enabled {
                <p role="status" class="mb-5 rounded-md border border-solid border-[#ffe58f] bg-[#fffbe6] px-4 py-3 text-sm text-[#ad6800]">"当前组已停用，本组所有 Key 暂时无法调用模型。重新启用组后可恢复。"</p>
            }
            <section class="overflow-hidden rounded-lg border border-solid border-border bg-white" aria-labelledby="key-list-title">
                <header class="flex flex-wrap items-center justify-between gap-4 px-6 py-5 max-[640px]:px-4">
                    <div>
                        <h2 id="key-list-title" class="m-0 text-base font-semibold">"虚拟 Key"<span class="ml-2 text-sm font-normal text-muted">(key_rows.len())</span></h2>
                        <p class="mb-0 mt-1 text-[13px] text-secondary">"用于模型 API 认证，不授予控制台管理权限。"</p>
                    </div>
                    <div class="flex min-w-0 flex-wrap items-center gap-3">
                        <button class=(class!(BUTTON, PRIMARY)) type="button" (new_key.clone())>
                            icon(data: PLUS_OUTLINED, size: 14)
                            "创建 Key"
                        </button>
                    </div>
                </header>
                if key_rows.is_empty() {
                    <div class="border-t border-border px-6 py-14 text-center">
                        icon(data: AUDIT_OUTLINED, size: 36, attrs: attributes! { class="text-[#bfbfbf]" aria-hidden="true" })
                        <h3 class="mb-2 mt-4 text-base font-medium">"为应用创建第一个 Key"</h3>
                        <p class="mb-6 mt-0 text-sm text-secondary">"为每个调用方分配独立凭据，按需授权模型和路由，随时停用或撤销。"</p>
                        <button class=(class!(BUTTON, PRIMARY)) type="button" (new_key)>"创建 Key"</button>
                    </div>
                } else {
                    data_table(
                        label: "虚拟 Key 列表",
                        density: DataTableDensity::Compact,
                        attrs: attributes! { class="min-w-[840px] [&_th]:px-6! [&_td]:px-6!" },
                        <thead><tr><th>"名称 / 密钥"</th><th>"授权范围"</th><th>"状态"</th><th>"有效期"</th><th class="text-right!">"操作"</th></tr></thead>
                        <tbody>
                            #[key(key.id)]
                            for key in &key_rows[page_range] {
                                <tr>
                                    <td>
                                        <strong class="block max-w-[260px] truncate font-medium" title=(key.name.as_str())>(key.name.as_str())</strong>
                                        <code class="mt-0.5 block text-[13px] text-muted">(format!("{}…", key.prefix))</code>
                                    </td>
                                    <td>
                                        <span>(if key.all_routes { "组内全部入口".to_owned() } else { format!("{} 个模型 · {} 个路由", key.model_ids.len(), key.route_ids.len()) })</span>
                                        if key.all_routes || (key.route_ids.is_empty() && key.model_ids.is_empty()) {
                                            <span class="mt-0.5 block text-[13px] text-secondary">
                                                (if key.all_routes { "含模型别名及未来新增入口" } else { "暂无调用权限" })
                                            </span>
                                        }
                                    </td>
                                    <td>
                                        tag(tone: key_status(key).1, (key_status(key).0))
                                    </td>
                                    <td class="whitespace-nowrap">
                                        (format_expiry(key.expires_at))
                                        if key.expires_at.is_some() { <span class="mt-0.5 block text-xs text-muted">"UTC+8"</span> }
                                    </td>
                                    <td>
                                        key_actions(key: key, group_id: store.group_id(), csrf: csrf.as_str(), controls: &controls)
                                    </td>
                                </tr>
                            }
                        </tbody>
                    )
                }
                pagination(state: &paging, total: key_rows.len(), id: "keys-page-size", label: "Keys 列表分页")
            </section>
            <p class="mb-0 mt-4 text-[13px] text-muted">"完整密钥仅在创建成功时展示一次，列表仅显示前缀。停用可恢复，撤销后不可恢复。"</p>
        </section>
    })
}

fn key_status(key: &VirtualKeyView) -> (&'static str, TagTone) {
    if key.revoked {
        ("已撤销", TagTone::Default)
    } else if key
        .expires_at
        .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp())
    {
        ("已过期", TagTone::Warning)
    } else if key.enabled {
        ("已启用", TagTone::Success)
    } else {
        ("已停用", TagTone::Default)
    }
}

fn format_expiry(expiry: Option<i64>) -> String {
    expiry
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|date| {
            date.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "永不过期".into())
}

#[component]
async fn key_actions(
    cx: &Cx,
    key: &VirtualKeyView,
    group_id: i64,
    csrf: &str,
    controls: &Controls,
) -> Result<impl View> {
    let dialog_id = format!("key-revoke-{}", key.id);
    let toggle_id = format!("key-toggle-form-{}", key.id);
    let revoke_id = format!("key-revoke-form-{}", key.id);
    let trigger = native_dialog_trigger_attributes(cx, &dialog_id);
    let close = native_dialog_close_attributes(cx, &dialog_id);
    let busy = signal(cx, || false);
    let open = signal(cx, || false);
    let error = signal(cx, String::new);
    let toggle = key_submit(cx, controls, &busy, &open, &error, &toggle_id);
    let revoke = key_submit(cx, controls, &busy, &open, &error, &revoke_id);
    Ok(view! {
        <div class="flex justify-end gap-4">
            if key.revoked { <span class="text-[13px] text-muted">"不可恢复"</span> }
            else {
                if !key.expires_at.is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp()) {
                    <form id=(toggle_id.as_str()) method="post" action=(href!(group::details, group::GroupId(group_id)).query([("tab", "keys")])) class="m-0" (toggle)>
                        <input type="hidden" name="csrf" value=(csrf)>
                        <input type="hidden" name="group_id" value=(group_id.to_string())>
                        <input type="hidden" name="id" value=(key.id.to_string())>
                        <input type="hidden" name="version" value=(key.version.to_string())>
                        <input type="hidden" name="action" value=(if key.enabled { "disable" } else { "enable" })>
                        <button class=(LINK) type="submit" :disabled=$(busy.get())>(if key.enabled { "停用" } else { "启用" })</button>
                    </form>
                }
                <button class="border-0 bg-transparent p-0 text-sm text-[#cf1322] hover:text-[#ff4d4f]" type="button" (trigger) :disabled=$(busy.get())>"撤销"</button>
                native_dialog(config: NativeDialogConfig::new(&dialog_id, "撤销虚拟 Key"), open: Some(&open), busy: &busy, language: UiLanguage::ChineseSimplified, attrs: attributes! { class=(DIALOG) },
                    <div class="px-6 py-5">
                        <p role="alert" class="mb-3 mt-0 text-sm text-[#cf1322]" :hidden=$(error.get().is_empty())>$(error.get())</p>
                        <p class="mb-3 mt-0 text-sm">"确认撤销「"(key.name.as_str())"」？"</p>
                        <p class="m-0 text-sm leading-relaxed text-secondary">"撤销后，这个 Key 将立即失效且无法恢复。调用方需要更换凭据。如果只是暂停使用，可以选择停用。"</p>
                    </div>
                    <form id=(revoke_id.as_str()) method="post" action=(href!(group::details, group::GroupId(group_id)).query([("tab", "keys")])) class=(FOOTER) (revoke)>
                        <input type="hidden" name="csrf" value=(csrf)>
                        <input type="hidden" name="group_id" value=(group_id.to_string())>
                        <input type="hidden" name="id" value=(key.id.to_string())>
                        <input type="hidden" name="version" value=(key.version.to_string())>
                        <input type="hidden" name="action" value="revoke">
                        <button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button>
                        <button class="inline-flex h-9 items-center rounded-md border border-solid border-[#ff4d4f] bg-[#ff4d4f] px-4 text-sm font-medium text-white hover:bg-[#ff7875]" type="submit" :disabled=$(busy.get())>"确认撤销"</button>
                    </form>
                )
            }
        </div>
    })
}

fn key_submit(
    cx: &Cx,
    controls: &Controls,
    busy: &Signal<bool>,
    open: &Signal<bool>,
    error: &Signal<String>,
    form_id: &str,
) -> Attributes {
    let Controls {
        refresh,
        success,
        failure,
    } = controls;
    let confirming = form_id.starts_with("key-revoke-form-");
    let unavailable: Outcome = Err("请求失败或结果未确认，请检查列表状态后重试".into());
    attributes! {
        cx =>
        @submit=$(async |event: Event| {
            event.prevent_default();
            if busy.get() { return; }
            if confirming { open.set(true); }
            busy.set(true);
            error.set("".to_owned());
            success.set("".to_owned());
            failure.set("".to_owned());
            let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById(${form_id}))).toString())", String::new());
            let result = raw!("await Promise.resolve(${change_key}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
            busy.set(false);
            if result.is_ok() {
                open.set(false);
                success.set(result.unwrap());
                refresh.increment();
            } else {
                let message = result.unwrap_err();
                error.set(message.clone());
                failure.set(message);
            }
        })
    }
}

#[derive(Deserialize)]
pub struct GroupForm {
    csrf: String,
    #[serde(default)]
    group_id: i64,
    #[serde(default)]
    version: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    return_to: String,
}

#[route(POST "/ui/groups/select")]
pub async fn select(cx: &Cx, Form(input): Form<GroupForm>) -> Result<Response> {
    check_csrf(cx, &input.csrf)?;
    if !crate::app::store(cx)
        .list_groups()
        .await?
        .iter()
        .any(|group| group.id == input.group_id)
    {
        return Err(topcoat::router::error::forbidden().into());
    }
    let mut response = Response::new(topcoat::router::Body::empty());
    *response.status_mut() = 303u16.try_into()?;
    let destination = match input.return_to.as_str() {
        "/ui/chat" => "/ui/chat".to_owned(),
        "/ui/keys" => "/ui/keys".to_owned(),
        "/ui/models" => "/ui/models".to_owned(),
        "/ui/routes" => "/ui/routes".to_owned(),
        _ => "/ui/groups".to_owned(),
    };
    response.headers_mut().insert(
        "location",
        crate::app::scoped_href(cx, &destination).parse()?,
    );
    if let Some(scope) = destination
        .strip_prefix("/ui/")
        .filter(|scope| matches!(*scope, "chat" | "keys" | "models" | "routes"))
    {
        response.headers_mut().insert(
            "set-cookie",
            format!(
                "llmproxy_{scope}_group={}; Path=/ui; HttpOnly; SameSite=Strict",
                input.group_id
            )
            .parse()?,
        );
    }
    Ok(response)
}

#[procedure("/ui/_topcoat/runtime/procedures/save-group")]
pub async fn save_group(cx: &Cx, payload: String) -> Result<Outcome> {
    let Form(input) = Form::<GroupForm>::from_bytes(payload.as_bytes())?;
    check_csrf(cx, &input.csrf)?;
    let store = crate::app::store(cx);
    let result = if input.group_id == 0 {
        store.create_group(&input.name).await
    } else {
        store
            .update_group(input.group_id, input.version, &input.name, input.enabled)
            .await
    };
    Ok(result
        .map(|_| {
            if input.group_id == 0 {
                "组已创建"
            } else {
                "组已更新"
            }
            .to_owned()
        })
        .map_err(|error| error.to_string()))
}

#[procedure("/ui/_topcoat/runtime/procedures/create-key")]
pub async fn create_key(cx: &Cx, payload: String) -> Result<Outcome> {
    let Form(fields) = Form::<Vec<(String, String)>>::from_bytes(payload.as_bytes())?;
    let value = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or("")
    };
    check_csrf(cx, value("csrf"))?;
    let store = crate::app::store(cx).for_group(value("group_id").parse::<i64>()?);
    let expires_at = if value("expires_at").is_empty() {
        None
    } else {
        match chrono::NaiveDateTime::parse_from_str(value("expires_at"), "%Y-%m-%dT%H:%M") {
            Ok(date) => Some(date.and_utc().timestamp() - 8 * 3600),
            Err(_) => return Ok(Err("请选择有效的过期时间".into())),
        }
    };
    let route_ids = fields
        .iter()
        .filter(|(name, _)| name == "route_id")
        .map(|(_, id)| id.parse())
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    let model_ids = fields
        .iter()
        .filter(|(name, _)| name == "model_id")
        .map(|(_, id)| id.parse())
        .collect::<std::result::Result<Vec<i64>, _>>()?;
    Ok(store
        .create_virtual_key(llmproxy_store::VirtualKeyInput {
            name: value("name").into(),
            all_routes: value("all_routes") == "true",
            route_ids,
            model_ids,
            expires_at,
        })
        .await
        .map(|created| created.secret)
        .map_err(|error| error.to_string()))
}

#[derive(Deserialize)]
pub struct KeyAction {
    csrf: String,
    group_id: i64,
    id: i64,
    version: u64,
    action: String,
}
#[procedure("/ui/_topcoat/runtime/procedures/change-key")]
pub async fn change_key(cx: &Cx, payload: String) -> Result<Outcome> {
    let Form(input) = Form::<KeyAction>::from_bytes(payload.as_bytes())?;
    check_csrf(cx, &input.csrf)?;
    let store = crate::app::store(cx).for_group(input.group_id);
    let (result, message) = match input.action.as_str() {
        "revoke" => (
            store.revoke_virtual_key(input.id, input.version).await,
            "Key 已撤销",
        ),
        "enable" | "disable" => (
            store
                .set_virtual_key_enabled(input.id, input.version, input.action == "enable")
                .await,
            if input.action == "enable" {
                "Key 已启用"
            } else {
                "Key 已停用"
            },
        ),
        _ => return Err(topcoat::router::error::forbidden().into()),
    };
    Ok(result
        .map(|_| message.to_owned())
        .map_err(|error| error.to_string()))
}
