use super::*;
use llmproxy_store::ModelRouteView;
use topcoat_ant_design::{FormFieldConfig, form_field};

#[component]
pub(super) async fn group_editor(
    cx: &Cx,
    group: Option<&GroupView>,
    controls: &Controls,
    csrf: &str,
) -> Result<impl View> {
    let id = group
        .map(|group| format!("group-edit-{}", group.id))
        .unwrap_or_else(|| "group-create".into());
    let form_id = format!("{id}-form");
    let open = signal(cx, || false);
    let busy = signal(cx, || false);
    let error = signal(cx, String::new);
    let close = native_dialog_close_attributes(cx, &id);
    let name_id = format!("{id}-name");
    let name = group.map(|g| g.name.as_str()).unwrap_or("");
    let enabled = group.is_some_and(|g| g.enabled);
    let Controls {
        refresh,
        success,
        failure,
    } = controls;
    let unavailable: Outcome = Err("保存请求失败或结果未确认，请检查列表状态后重试".into());
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new(&id, if group.is_some() { "编辑资源组" } else { "新增资源组" }),
            open: Some(&open), busy: &busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class=(DIALOG) },
            <form id=(form_id.as_str()) method="post" action="/ui/groups" class="m-0"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    open.set(true);
                    busy.set(true);
                    error.set("".to_owned());
                    success.set("".to_owned());
                    failure.set("".to_owned());
                    let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById(${form_id}))).toString())", String::new());
                    let result = raw!("await Promise.resolve(${save_group}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
                    busy.set(false);
                    if result.is_ok() {
                        open.set(false);
                        success.set(result.unwrap());
                        let _reset = raw!("document.getElementById(${form_id}).reset()", ());
                        refresh.increment();
                    } else {
                        error.set(result.unwrap_err());
                    }
                })
            >
                <input type="hidden" name="csrf" value=(csrf)>
                if let Some(group) = group {
                    <input type="hidden" name="group_id" value=(group.id.to_string())>
                    <input type="hidden" name="version" value=(group.version.to_string())>
                }
                <div class="grid gap-5 px-6 py-5">
                    <p role="alert" class="m-0 rounded-md border border-solid border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]" :hidden=$(error.get().is_empty())>$(error.get())</p>
                    form_field(config: FormFieldConfig::new(&name_id, "组名称").required(),
                        <input id=(name_id.as_str()) class="w-full" name="name" value=(name) placeholder="例如：研发团队、生产应用" required=(true) maxlength="80" autofocus="">
                    )
                    if group.is_some() {
                        <div>
                            <label class="inline-flex items-center gap-2 text-sm">
                                <input type="checkbox" name="enabled" value="true" checked=(enabled)>
                                "启用此组"
                            </label>
                            <p class="mb-0 mt-2 text-[13px] leading-relaxed text-secondary">"停用组会立即阻止本组所有 Key 的模型调用，重新启用后可恢复。"</p>
                        </div>
                    } else {
                        <p class="m-0 text-[13px] leading-relaxed text-secondary">"创建后进入组内「模型」添加已有模型或路由，再进入「Keys」为应用分配凭据。"</p>
                    }
                </div>
                <footer class=(FOOTER)>
                    <button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button>
                    <button class=(class!(BUTTON, PRIMARY)) type="submit" :disabled=$(busy.get())>(if group.is_some() { "保存修改" } else { "创建组" })</button>
                </footer>
            </form>
        )
    })
}

#[component]
pub(super) async fn key_editor(
    cx: &Cx,
    group_id: i64,
    models: &[llmproxy_store::ModelMappingView],
    routes: &[ModelRouteView],
    controls: &Controls,
    csrf: &str,
    secret: &Signal<String>,
    created: &Signal<bool>,
) -> Result<impl View> {
    let group_name = app_context::<AppState>(cx)
        .store
        .list_groups()
        .await?
        .into_iter()
        .find(|g| g.id == group_id)
        .map(|g| g.name)
        .unwrap_or_default();
    let open = signal(cx, || false);
    let busy = signal(cx, || false);
    let error = signal(cx, String::new);
    let all_routes = signal(cx, || false);
    let resource_tab = signal(cx, || "models".to_owned());
    let close = native_dialog_close_attributes(cx, "key-create");
    let Controls {
        refresh,
        success,
        failure,
    } = controls;
    let unavailable: Outcome = Err("创建请求失败或结果未确认，请检查列表状态后重试".into());
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new("key-create", "创建虚拟 Key"),
            open: Some(&open), busy: &busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class=(class!(DIALOG, "w-[min(640px,calc(100%_-_32px))]!")) },
            <form id="key-editor-form" method="post" action=(href!(group::details, group::GroupId(group_id)).query([("tab", "keys")])) class="m-0 flex min-h-0 flex-col" autocomplete="off"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    open.set(true);
                    busy.set(true);
                    error.set("".to_owned());
                    success.set("".to_owned());
                    failure.set("".to_owned());
                    let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById('key-editor-form'))).toString())", String::new());
                    let result = raw!("await Promise.resolve(${create_key}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
                    busy.set(false);
                    if result.is_ok() {
                        open.set(false);
                        secret.set(result.unwrap());
                        created.set(true);
                        all_routes.set(false);
                        resource_tab.set("models".to_owned());
                        let _reset = raw!("document.getElementById('key-editor-form').reset()", ());
                        refresh.increment();
                    } else {
                        error.set(result.unwrap_err());
                    }
                })
            >
                <input type="hidden" name="csrf" value=(csrf)>
                <input type="hidden" name="group_id" value=(group_id.to_string())>
                <div class="grid min-h-0 gap-5 overflow-y-auto overscroll-contain px-6 py-5 max-[640px]:px-4">
                    <p role="alert" class="m-0 rounded-md border border-solid border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]" :hidden=$(error.get().is_empty())>$(error.get())</p>
                    <p class="m-0 text-sm text-secondary">"所属资源组："<strong class="text-heading">(group_name.as_str())</strong><span class="ml-2 text-xs text-muted">"每个 Key 只属于一个组"</span></p>
                    <div class="grid grid-cols-2 items-start gap-5 max-[640px]:grid-cols-1 [&>div]:content-start">
                        form_field(config: FormFieldConfig::new("key-name", "名称").required(),
                            <input id="key-name" class="w-full" name="name" placeholder="例如：网站后端、开发测试" required=(true) maxlength="80" autofocus="">
                        )
                        form_field(config: FormFieldConfig::new("key-expiry", "过期时间（可选）").with_hint("UTC+8；留空表示永不过期"),
                            <input id="key-expiry" class="w-full" name="expires_at" type="datetime-local" aria-describedby="key-expiry-help">
                        )
                    </div>
                    <fieldset class="group-permission m-0 min-w-0 border-0 p-0">
                        <legend class="mb-3 p-0 text-sm font-medium">"授权范围"</legend>
                        <div class="grid grid-cols-2 gap-3 max-[640px]:grid-cols-1">
                            <label class="flex cursor-pointer items-start gap-3 rounded-md border border-solid border-control-border px-4 py-3 has-[:checked]:border-primary has-[:checked]:bg-primary-soft">
                                <input type="radio" name="all_routes" value="false" checked=(true) @change=$(|_event: Event| all_routes.set(false))>
                                <span><strong class="block text-sm font-medium">"指定资源"</strong><span class="mt-1 block text-[13px] leading-relaxed text-secondary">"仅允许调用下方勾选的模型和路由。"</span></span>
                            </label>
                            <label class="flex cursor-pointer items-start gap-3 rounded-md border border-solid border-control-border px-4 py-3 has-[:checked]:border-primary has-[:checked]:bg-primary-soft">
                                <input type="radio" name="all_routes" value="true" checked=(false) @change=$(|_event: Event| all_routes.set(true))>
                                <span><strong class="block text-sm font-medium">"组内全部入口"</strong><span class="mt-1 block text-[13px] leading-relaxed text-secondary">"允许本组全部模型和路由，包含未来加入的资源。"</span></span>
                            </label>
                        </div>
                    </fieldset>
                    <fieldset class="m-0 min-w-0 rounded-md border border-solid border-border p-4" :hidden=$(all_routes.get()) :disabled=$(all_routes.get())>
                        <legend class="px-1 text-sm font-medium">"可调用资源"</legend>
                        if models.is_empty() && routes.is_empty() {
                            <p class="m-0 text-sm text-secondary">"当前组还没有添加模型或路由。"<a href=(href!(group::details, group::GroupId(group_id))) class="ml-2 text-primary hover:text-primary-hover">"前往添加模型或路由"</a></p>
                        } else {
                            <div role="group" aria-label="可调用资源类型" class="mb-3 flex gap-6 border-b border-border">
                                <button type="button" class="border-0 border-b-2! border-solid! border-transparent bg-transparent px-0 pb-3 text-sm text-secondary aria-pressed:border-primary! aria-pressed:text-primary" :aria-pressed=$(resource_tab.get() == "models") @click=$(|_event: Event| resource_tab.set("models".to_owned()))>"Models "(models.len())</button>
                                <button type="button" class="border-0 border-b-2! border-solid! border-transparent bg-transparent px-0 pb-3 text-sm text-secondary aria-pressed:border-primary! aria-pressed:text-primary" :aria-pressed=$(resource_tab.get() == "routes") @click=$(|_event: Event| resource_tab.set("routes".to_owned()))>"Model Routes "(routes.len())</button>
                            </div>
                            <div class="grid max-h-[220px] gap-1 overflow-y-auto" :hidden=$(resource_tab.get() != "models")>
                                if models.is_empty() { <p class="my-3 text-sm text-secondary">"当前组还没有添加模型。"</p> }
                                #[key(model.id)]
                                for model in models {
                                    <label class="flex cursor-pointer items-start gap-3 rounded px-2 py-2 hover:bg-surface">
                                        <input type="checkbox" name="model_id" value=(model.id.to_string())>
                                        <span class="min-w-0"><span class="block break-all text-sm">(model.alias.as_str())</span><span class="mt-0.5 block break-all text-xs text-muted">(model.provider_name.as_str())" · "(model.upstream_model_id.as_str())</span></span>
                                    </label>
                                }
                            </div>
                            <div class="grid max-h-[220px] gap-1 overflow-y-auto" :hidden=$(resource_tab.get() != "routes")>
                                if routes.is_empty() { <p class="my-3 text-sm text-secondary">"当前组还没有添加路由。"</p> }
                                #[key(route.id)]
                                for route in routes {
                                    <label class="flex cursor-pointer items-start gap-3 rounded px-2 py-2 hover:bg-surface">
                                        <input type="checkbox" name="route_id" value=(route.id.to_string())>
                                        <span class="min-w-0"><span class="block break-all text-sm">(route.name.clone())</span><span class="mt-0.5 block text-xs text-muted">(super::super::providers::protocol_label(route.protocol))</span></span>
                                    </label>
                                }
                            </div>
                        }
                        <p class="mb-0 mt-3 text-[13px] text-secondary">"未选择资源的 Key 没有调用权限；选择路由不会自动授权候选模型。"</p>
                    </fieldset>
                    <p class="m-0 text-[13px] leading-relaxed text-secondary">"Key 只用于模型 API 认证。创建成功后，请立即复制并妥善保存完整密钥。"</p>
                </div>
                <footer class=(FOOTER)>
                    <button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button>
                    <button class=(class!(BUTTON, PRIMARY)) type="submit" :disabled=$(busy.get())>"创建 Key"</button>
                </footer>
            </form>
        )
    })
}

#[component]
pub(super) async fn secret_dialog(
    cx: &Cx,
    secret: &Signal<String>,
    open: &Signal<bool>,
) -> Result<impl View> {
    let busy = signal(cx, || false);
    let copy_label = signal(cx, || "复制 Key".to_owned());
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new("key-created", "Key 已创建"),
            open: Some(open), busy: &busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! {
                cx =>
                class=(DIALOG)
                @close=$(|_event: Event| { secret.set("".to_owned()); copy_label.set("复制 Key".to_owned()); })
            },
            <div class="px-6 py-5">
                <p class="mb-4 mt-0 text-sm leading-relaxed">"完整密钥只展示这一次。请立即复制保存，关闭后无法再次查看。"</p>
                <pre id="created-key-secret" class="m-0 select-all whitespace-pre-wrap break-all rounded-md border border-solid border-control-border bg-surface p-4 text-sm leading-relaxed">$(secret.get())</pre>
                <p role="alert" class="mb-0 mt-3 text-[13px] text-[#cf1322]" :hidden=$(copy_label.get() != "复制失败")>"复制失败，请选择密钥并手动复制。"</p>
                <dl class="mb-0 mt-4 grid grid-cols-[80px_minmax(0,1fr)] gap-x-3 gap-y-2 text-[13px] text-secondary">
                    <dt>"OpenAI"</dt><dd class="m-0 break-all"><code>"Authorization: Bearer <Key>"</code></dd>
                    <dt>"Anthropic"</dt><dd class="m-0 break-all"><code>"x-api-key: <Key>"</code></dd>
                    <dt>"Gemini"</dt><dd class="m-0 break-all"><code>"x-goog-api-key: <Key>"</code></dd>
                </dl>
            </div>
            <footer class=(FOOTER)>
                <button type="button" class=(BUTTON) @click=$(|_event: Event| { open.set(false); secret.set("".to_owned()); })>"已保存，关闭"</button>
                <button class=(class!(BUTTON, PRIMARY)) type="button" @click=$(async |_event: Event| {
                    let copied = raw!("await Promise.resolve().then(() => navigator.clipboard.writeText(document.getElementById('created-key-secret').textContent)).then(() => true).catch(() => false)", false);
                    copy_label.set(if copied { "已复制" } else { "复制失败" }.to_owned());
                })>$(copy_label.get())</button>
                <span role="status" class="sr-only" aria-live="polite">$(copy_label.get())</span>
            </footer>
        )
    })
}
