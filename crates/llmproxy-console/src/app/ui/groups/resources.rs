use super::*;
use llmproxy_store::{ModelMappingView, ModelRouteView};

enum Resource {
    Model(Box<ModelMappingView>),
    Route(ModelRouteView),
}

impl Resource {
    fn key(&self) -> String {
        match self {
            Self::Model(model) => format!("model-{}", model.id),
            Self::Route(route) => format!("route-{}", route.id),
        }
    }
}

#[component]
pub(super) async fn resource_workspace(
    cx: &Cx,
    group_id: i64,
    controls: &Controls,
) -> Result<impl View> {
    let group = find_group(cx, group_id).await?;
    let store = app_context::<AppState>(cx).store.for_group(group_id);
    let models = store.list_all_models().await?;
    let routes = store.list_all_routes().await?;
    let model_count = models
        .iter()
        .filter(|model| model.group_ids.contains(&group_id))
        .count();
    let route_count = routes
        .iter()
        .filter(|route| route.group_ids.contains(&group_id))
        .count();
    let csrf = app_context::<AppState>(cx).csrf.clone();
    let query = signal(cx, String::new);
    let kind = signal(cx, String::new);
    let paging = Pagination::new(cx);
    let page = paging.page.clone();
    let needle = query.get().trim().to_lowercase();
    let selected_kind = kind.get();
    let filtered: Vec<_> = models
        .iter()
        .filter(|model| model.group_ids.contains(&group_id))
        .filter(|model| {
            selected_kind != "route"
                && format!(
                    "{} {} {}",
                    model.alias, model.upstream_model_id, model.provider_name
                )
                .to_lowercase()
                .contains(&needle)
        })
        .cloned()
        .map(|model| Resource::Model(Box::new(model)))
        .chain(
            routes
                .iter()
                .filter(|route| route.group_ids.contains(&group_id))
                .filter(|route| {
                    selected_kind != "model"
                        && format!(
                            "{} {}",
                            route.name,
                            route
                                .targets
                                .iter()
                                .map(|target| format!(
                                    "{} {} {}",
                                    target.model.alias,
                                    target.model.upstream_model_id,
                                    target.model.provider_name
                                ))
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                        .to_lowercase()
                        .contains(&needle)
                })
                .cloned()
                .map(Resource::Route),
        )
        .collect();
    let page_range = paging.range(filtered.len());
    let new_resources =
        native_dialog_trigger_attributes(cx, &format!("group-resources-{group_id}"));
    Ok(view! {
        <section class="w-full min-w-0">
            group_heading(group: &group, is_keys: false)
            <section class="min-w-0 overflow-hidden rounded-lg border border-solid border-border bg-white" aria-labelledby="group-models-title">
                <header class="flex flex-wrap items-center justify-between gap-4 px-6 py-5 max-[640px]:px-4">
                    <div>
                        <h2 id="group-models-title" class="m-0 text-base font-semibold">"模型与路由"</h2>
                        <p class="mb-0 mt-1 text-[13px] text-secondary">(format!("{} 个模型 · {} 个路由", model_count, route_count))</p>
                    </div>
                    <div class="flex min-w-0 flex-wrap items-center gap-3">
                        <select aria-label="资源类型" class="h-9!" :value=$(kind.get()) @change=$(|event: Event| { kind.set(event.target.value); page.set(1); })>
                            <option value="">"全部类型"</option><option value="model">"Models"</option><option value="route">"Model Routes"</option>
                        </select>
                        <input type="search" class="w-[240px] max-w-full" aria-label="搜索组内资源" placeholder="搜索名称或 Provider" :value=$(query.get()) @input=$(|event: Event| { query.set(event.target.value); page.set(1); })>
                        <button type="button" class=(class!(BUTTON, PRIMARY)) (new_resources.clone())>icon(data: PLUS_OUTLINED, size: 14)"添加模型或路由"</button>
                    </div>
                </header>
                if model_count == 0 && route_count == 0 {
                    <div class="border-t border-border px-6 py-14 text-center">
                        <h3 class="mb-2 mt-0 text-base font-medium">"当前组还没有模型或路由"</h3>
                        <p class="mb-6 mt-0 text-sm text-secondary">"从系统中选择已导入的模型或已创建的路由加入此组，然后为 Key 授权。"</p>
                        <button type="button" class=(class!(BUTTON, PRIMARY)) (new_resources)>"添加模型或路由"</button>
                    </div>
                } else {
                    data_table(label: "组内模型与路由", density: DataTableDensity::Compact, attrs: attributes! { class="min-w-[680px] [&_th]:px-6! [&_td]:px-6!" },
                        <thead><tr><th>"名称"</th><th>"类型"</th><th>"资源详情"</th><th>"状态"</th><th class="text-right!">"操作"</th></tr></thead>
                        <tbody>
                            if filtered.is_empty() { <tr><td colspan="5" class="py-8! text-center! text-muted">"没有匹配的模型或路由"</td></tr> }
                            #[key(resource.key())]
                            for resource in &filtered[page_range] {
                                match resource {
                                    Resource::Model(model) => {
                                        <tr data-group-model=(model.id.to_string())>
                                            <td><strong class="font-medium">(model.alias.as_str())</strong></td>
                                            <td>tag(tone: TagTone::Default, "Model")</td>
                                            <td><span class="block">(model.provider_name.as_str())</span><span class="mt-0.5 block text-xs text-muted">(model.upstream_model_id.as_str())</span></td>
                                            <td>tag(tone: if model.provider_enabled { TagTone::Success } else { TagTone::Default }, (if model.provider_enabled { "已启用" } else { "Provider 已停用" }))</td>
                                            <td>remove_action(group: &group, id: model.id, kind: "model", name: model.alias.as_str(), controls: controls, csrf: csrf.as_str())</td>
                                        </tr>
                                    }
                                    Resource::Route(route) => {
                                        <tr data-group-route=(route.id.to_string())>
                                            <td><strong class="font-medium">(route.name.as_str())</strong></td>
                                            <td>tag(tone: TagTone::Default, "Model Route")</td>
                                            <td><span class="block">(super::super::providers::protocol_label(route.protocol))" → "(super::super::providers::protocol_label(route.provider_protocol))</span><span class="mt-0.5 block text-xs text-muted">(format!("{} 个候选模型", route.targets.len()))</span></td>
                                            <td>tag(tone: if route.enabled { TagTone::Success } else { TagTone::Default }, (if route.enabled { "已启用" } else { "已停用" }))</td>
                                            <td>remove_action(group: &group, id: route.id, kind: "route", name: route.name.as_str(), controls: controls, csrf: csrf.as_str())</td>
                                        </tr>
                                    }
                                }
                            }
                        </tbody>
                    )
                }
                pagination(state: &paging, total: filtered.len(), id: "group-models-page-size", label: "组内模型与路由分页")
            </section>
            <p class="mb-0 mt-4 text-[13px] text-muted">"这里只管理本组关联；模型与路由的配置在系统 Models / Model Routes 中维护。"</p>
        </section>
        resource_editor(group: &group, models: &models, routes: &routes, controls: controls, csrf: csrf.as_str())
    })
}

#[component]
async fn remove_action(
    cx: &Cx,
    group: &GroupView,
    id: i64,
    kind: &str,
    name: &str,
    controls: &Controls,
    csrf: &str,
) -> Result<impl View> {
    let dialog_id = format!("group-remove-{kind}-{id}");
    let form_id = format!("{dialog_id}-form");
    let open = signal(cx, || false);
    let busy = signal(cx, || false);
    let error = signal(cx, String::new);
    let close = native_dialog_close_attributes(cx, &dialog_id);
    let Controls {
        refresh,
        success,
        failure,
    } = controls;
    let unavailable: Outcome = Err("移出结果未确认，请刷新列表后重试".into());
    Ok(view! {
        <div class="flex justify-end">
            <button type="button" class=(LINK) (native_dialog_trigger_attributes(cx, &dialog_id))>"移出组"</button>
        </div>
        native_dialog(config: NativeDialogConfig::new(&dialog_id, "移出资源组"), open: Some(&open), busy: &busy, language: UiLanguage::ChineseSimplified, attrs: attributes! { class=(DIALOG) },
            <form id=(form_id.as_str()) class="m-0" method="post" action=(href!(group::group_models, group::GroupId(group.id)))
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    open.set(true); busy.set(true); error.set("".to_owned()); success.set("".to_owned()); failure.set("".to_owned());
                    let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById(${form_id}))).toString())", String::new());
                    let result = raw!("await Promise.resolve(${remove_group_resource}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
                    busy.set(false);
                    if result.is_ok() { open.set(false); success.set(result.unwrap()); refresh.increment(); }
                    else { error.set(result.unwrap_err()); }
                })
            >
                <input type="hidden" name="csrf" value=(csrf)><input type="hidden" name="group_id" value=(group.id.to_string())>
                <input type="hidden" name="version" value=(group.version.to_string())><input type="hidden" name="id" value=(id.to_string())><input type="hidden" name="kind" value=(kind)>
                <div class="px-6 py-5">
                    <p role="alert" class="mb-3 mt-0 text-sm text-[#cf1322]" :hidden=$(error.get().is_empty())>$(error.get())</p>
                    <p class="mt-0 text-sm">"将「"(name)"」移出「"(group.name.as_str())"」？"</p>
                    <p class="mb-0 text-sm leading-relaxed text-secondary">"不会删除系统资源；本组 Key 对该资源的指定授权会清除，重新加入后需重新授权。"</p>
                </div>
                <footer class=(FOOTER)><button type="button" class=(BUTTON) (close) :disabled=$(busy.get())>"取消"</button><button type="submit" class=(class!(BUTTON, PRIMARY)) :disabled=$(busy.get())>"移出组"</button></footer>
            </form>
        )
    })
}

#[derive(Deserialize)]
pub struct RemoveResource {
    csrf: String,
    group_id: i64,
    version: u64,
    id: i64,
    kind: String,
}

#[procedure("/ui/_topcoat/runtime/procedures/remove-group-resource")]
pub async fn remove_group_resource(cx: &Cx, payload: String) -> Result<Outcome> {
    let Form(input) = Form::<RemoveResource>::from_bytes(payload.as_bytes())?;
    check_csrf(cx, &input.csrf)?;
    let store = app_context::<AppState>(cx).store.for_group(input.group_id);
    let mut models: Vec<_> = store
        .list_models()
        .await?
        .into_iter()
        .map(|model| model.id)
        .collect();
    let mut routes: Vec<_> = store
        .list_routes()
        .await?
        .into_iter()
        .map(|route| route.id)
        .collect();
    let ids = match input.kind.as_str() {
        "model" => &mut models,
        "route" => &mut routes,
        _ => return Err(topcoat::router::error::forbidden().into()),
    };
    if !ids.contains(&input.id) {
        return Ok(Err("资源已移出，请刷新后重试".into()));
    }
    ids.retain(|id| *id != input.id);
    Ok(store
        .set_group_resources(input.version, models, routes)
        .await
        .map(|_| "资源已移出组".to_owned())
        .map_err(|error| error.to_string()))
}

#[component]
pub(super) async fn resource_editor(
    cx: &Cx,
    group: &GroupView,
    models: &[ModelMappingView],
    routes: &[ModelRouteView],
    controls: &Controls,
    csrf: &str,
) -> Result<impl View> {
    let id = format!("group-resources-{}", group.id);
    let form_id = format!("{id}-form");
    let title = format!("添加模型或路由 · {}", group.name);
    let open = signal(cx, || false);
    let busy = signal(cx, || false);
    let error = signal(cx, String::new);
    let query = signal(cx, String::new);
    let tab = signal(cx, || "models".to_owned());
    let close = native_dialog_close_attributes(cx, &id);
    let Controls {
        refresh,
        success,
        failure,
    } = controls;
    let unavailable: Outcome = Err("保存结果未确认，请检查列表后重试".into());
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new(&id, &title),
            open: Some(&open), busy: &busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class=(class!(DIALOG, "w-[min(720px,calc(100%_-_32px))]!")) },
            <form id=(form_id.as_str()) method="post" action=(href!(group::group_models, group::GroupId(group.id))) class="m-0 flex min-h-0 flex-col"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() { return; }
                    open.set(true);
                    busy.set(true);
                    error.set("".to_owned());
                    success.set("".to_owned());
                    failure.set("".to_owned());
                    let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById(${form_id}))).toString())", String::new());
                    let result = raw!("await Promise.resolve(${save_group_resources}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
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
                <input type="hidden" name="csrf" value=(csrf)>
                <input type="hidden" name="group_id" value=(group.id.to_string())>
                <input type="hidden" name="version" value=(group.version.to_string())>
                for model in models.iter().filter(|model| model.group_ids.contains(&group.id)) {
                    <input type="hidden" name="model_id" value=(model.id.to_string())>
                }
                for route in routes.iter().filter(|route| route.group_ids.contains(&group.id)) {
                    <input type="hidden" name="route_id" value=(route.id.to_string())>
                }
                <div class="grid min-h-0 gap-4 overflow-y-auto overscroll-contain px-6 py-5 max-[640px]:px-4">
                    <p role="alert" class="m-0 rounded-md border border-solid border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]" :hidden=$(error.get().is_empty())>$(error.get())</p>
                    <p class="m-0 text-sm leading-relaxed text-secondary">"选择要加入本组的已有模型或路由。同一资源可加入多个组，已加入的资源无需重复选择。"</p>
                    <div role="group" aria-label="资源类型" class="flex gap-6 border-b border-border">
                        <button type="button"  class="border-0 border-b-2! border-solid! border-transparent bg-transparent px-0 pb-3 text-sm text-secondary aria-pressed:border-primary! aria-pressed:text-primary" :aria-pressed=$(tab.get() == "models") @click=$(|_event: Event| tab.set("models".to_owned()))>"模型"</button>
                        <button type="button"  class="border-0 border-b-2! border-solid! border-transparent bg-transparent px-0 pb-3 text-sm text-secondary aria-pressed:border-primary! aria-pressed:text-primary" :aria-pressed=$(tab.get() == "routes") @click=$(|_event: Event| tab.set("routes".to_owned()))>"模型路由"</button>
                    </div>
                    <input type="search" class="w-full" placeholder="搜索名称、模型 ID 或 Provider" aria-label="搜索可加入的资源" :value=$(query.get()) @input=$(|event: Event| query.set(event.target.value))>
                    <div  aria-label="模型" :hidden=$(tab.get() != "models")>
                        <p class="mb-3 mt-0 text-[13px] text-secondary">"加入后，本组「全部入口」Key 可通过模型标识直接调用。"</p>
                        if models.is_empty() {
                            <p class="my-8 text-center text-sm text-muted">"还没有模型，请先在 Models 页面创建或导入。"</p>
                        }
                        <div class="grid max-h-[320px] gap-1 overflow-y-auto">
                            #[key(model.id)]
                            for model in models {
                                let search = format!("{} {} {}", model.alias, model.upstream_model_id, model.provider_name).to_lowercase();
                                <label class="flex cursor-pointer items-start gap-3 rounded-md px-3 py-3 hover:bg-surface has-[:checked]:bg-primary-soft" :hidden=$(if query.get().is_empty() { false } else { raw!("!${search}.dehydrate().includes(${query}.get().dehydrate().toLowerCase())", false) })>
                                    <input class="mt-1" type="checkbox" name="model_id" value=(model.id.to_string()) checked=(model.group_ids.contains(&group.id)) disabled=(model.group_ids.contains(&group.id))>
                                    <span class="min-w-0"><strong class="block break-all text-sm font-medium">(model.alias.as_str()) if model.group_ids.contains(&group.id) { <span class="ml-2 text-xs font-normal text-muted">"已加入"</span> }</strong><span class="mt-1 block break-all text-xs text-secondary">(model.provider_name.as_str())" · "(model.upstream_model_id.as_str())</span></span>
                                </label>
                            }
                        </div>
                    </div>
                    <div  aria-label="模型路由" :hidden=$(tab.get() != "routes")>
                        <p class="mb-3 mt-0 text-[13px] leading-relaxed text-secondary">"路由可引用系统中已导入的模型，无需把候选模型加入本组。加入后可为本组 Key 授权此路由。"</p>
                        if routes.is_empty() {
                            <p class="my-8 text-center text-sm text-muted">"还没有路由，请先在 Model Routes 页面创建。"</p>
                        }
                        <div class="grid max-h-[320px] gap-1 overflow-y-auto">
                            #[key(route.id)]
                            for route in routes {
                                let search = format!("{} {}", route.name, route.targets.iter().map(|target| format!("{} {} {}", target.model.alias, target.model.upstream_model_id, target.model.provider_name)).collect::<Vec<_>>().join(" ")).to_lowercase();
                                <label class="flex cursor-pointer items-start gap-3 rounded-md px-3 py-3 hover:bg-surface has-[:checked]:bg-primary-soft" :hidden=$(if query.get().is_empty() { false } else { raw!("!${search}.dehydrate().includes(${query}.get().dehydrate().toLowerCase())", false) })>
                                    <input class="mt-1" type="checkbox" name="route_id" value=(route.id.to_string()) checked=(route.group_ids.contains(&group.id)) disabled=(route.group_ids.contains(&group.id))>
                                    <span class="min-w-0"><strong class="block break-all text-sm font-medium">(route.name.as_str()) if route.group_ids.contains(&group.id) { <span class="ml-2 text-xs font-normal text-muted">"已加入"</span> }</strong><span class="mt-1 block text-xs text-secondary">(super::super::providers::protocol_label(route.protocol))" · "(route.targets.len())" 个候选模型"</span></span>
                                </label>
                            }
                        </div>
                    </div>
                    <p class="m-0 text-xs leading-relaxed text-muted">"模型与路由配置由各组共享，编辑配置会影响所有引用它的组。"</p>
                </div>
                <footer class=(FOOTER)>
                    <button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button>
                    <button class=(class!(BUTTON, PRIMARY)) type="submit" :disabled=$(busy.get())>"添加到组"</button>
                </footer>
            </form>
        )
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/save-group-resources")]
pub async fn save_group_resources(cx: &Cx, payload: String) -> Result<Outcome> {
    let Form(fields) = Form::<Vec<(String, String)>>::from_bytes(payload.as_bytes())?;
    let value = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    check_csrf(cx, value("csrf"))?;
    let group_id = value("group_id").parse::<i64>()?;
    let version = value("version").parse::<u64>()?;
    let ids = |name: &str| -> std::result::Result<Vec<i64>, std::num::ParseIntError> {
        fields
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, v)| v.parse())
            .collect()
    };
    let result = app_context::<AppState>(cx)
        .store
        .for_group(group_id)
        .set_group_resources(version, ids("model_id")?, ids("route_id")?)
        .await;
    Ok(result
        .map(|_| "组资源已更新".to_owned())
        .map_err(|error| error.to_string()))
}
