use super::providers::{BUTTON, PAGE_HEADING, PROTOCOLS, protocol_label};
use crate::app::check_csrf;
use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ModelRouteInput, ModelRouteTargetInput, ModelRouteView, StoreError};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, page},
    runtime::{Event, procedure, signal},
    view::{View, class, view},
};
use topcoat_ant_design::{TagTone, tag};

const PRIMARY: &str =
    "border-primary! bg-primary! text-white! hover:border-primary-hover! hover:bg-primary-hover!";
type Outcome = std::result::Result<String, String>;

#[derive(Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: String,
}

#[page]
pub async fn routes(cx: &Cx, Form(query): Form<ListQuery>) -> Result<impl View> {
    use topcoat::view::ViewExt;
    if !crate::app::store(cx).can_manage_resources() {
        return Ok(super::resource_catalog::member_catalog(cx, true)
            .await?
            .boxed());
    }

    let all = crate::app::store(cx).list_all_routes().await?;
    let needle = query.q.trim().to_lowercase();
    let filtered: Vec<_> = all
        .iter()
        .filter(|route| {
            needle.is_empty()
                || route.name.to_lowercase().contains(&needle)
                || route.targets.iter().any(|target| {
                    target.model.provider_name.to_lowercase().contains(&needle)
                        || target
                            .model
                            .upstream_model_id
                            .to_lowercase()
                            .contains(&needle)
                })
        })
        .cloned()
        .collect();
    let total = all.len();
    let error = signal(cx, String::new);
    let csrf = crate::app::auth::csrf_token(cx);
    let unavailable: Outcome = Err("删除失败，请重试".into());
    Ok(view! {
        <section class=(PAGE_HEADING)>
            <div>
                <h1 class="sr-only">"Model Routes"</h1>
                <p>
                    "使用系统中已导入的模型配置路由，再到资源组中添加。新建路由不会自动加入任何组。"
                </p>
            </div>
            <a
                class=(class!(BUTTON, PRIMARY))
                (topcoat::runtime::link_attrs(
                    cx, crate::app::scoped_href(cx, "/ui/routes/edit"),
                    topcoat::runtime::prefetch_mode(cx),
                ))
            >
                "＋ 新建路由"
            </a>
        </section>
        <section
            class="overflow-hidden rounded-lg border border-border bg-white shadow-xs"
            aria-label="模型路由列表"
        >
            <div
                class="flex flex-wrap items-center justify-between gap-4 px-6 py-5 max-[640px]:px-4"
            >
                <div>
                <h2 class="m-0 text-base font-semibold">
                    "路由列表"
                    <span
                        class="ml-2 rounded bg-surface px-2 text-[13px] font-normal text-secondary"
                    >
                        (total)
                    </span>
                </h2>
                <p class="mb-0 mt-1 text-[13px] text-secondary">
                    "此处统一维护系统路由；加入资源组后，该组 Key 才可调用"
                </p>
                </div>
                <div class="flex min-w-0 flex-wrap items-center gap-4 max-[640px]:w-full">
                    if total > 0 {
                        <form
                            class="m-0 flex w-[360px] max-w-full min-w-0 gap-2 max-[640px]:w-full"
                            method="get"
                            action="/ui/routes"
                            role="search"
                        >
 <input type="hidden" name="space" value=(crate::app::store(cx).space_id().unwrap().to_string())>
                            <input
                                class="h-9 min-w-0 flex-1 rounded-md border border-control-border px-3 text-sm focus:border-primary"
                                type="search"
                                name="q"
                                value=(query.q.as_str())
                                placeholder="搜索路由、上游模型或 Provider"
                                aria-label="搜索模型路由"
                            >
                            <button class=(BUTTON) type="submit">"搜索"</button>
                        </form>
                    }
                    <a href=(crate::app::scoped_href(cx, "/ui/groups")) class="whitespace-nowrap text-[13px] text-primary hover:underline">"管理组资源 →"</a>
                </div>
            </div>
            <p
                class="m-0 border-t border-border bg-[#fff2f0] px-6 py-3 text-sm text-[#cf1322]"
                role="alert"
                :hidden=$(error.get().is_empty())
            >
                $(error.get())
            </p>
            if total == 0 {
                <div class="border-t border-border px-6 py-16 text-center">
                    <h3 class="m-0 text-base font-medium">"还没有模型路由"</h3>
                    <p class="mt-2 text-sm text-secondary">
                        "先在 Models 中添加具体模型，再创建对外路由。"
                    </p>
                    <a
                        class=(class!(BUTTON, PRIMARY, "mt-4"))
                        (topcoat::runtime::link_attrs(
                            cx, crate::app::scoped_href(cx, "/ui/routes/edit"),
                            topcoat::runtime::prefetch_mode(cx),
                        ))
                    >
                        "新建路由"
                    </a>
                </div>
            } else if filtered.is_empty() {
                <div
                    class="border-t border-border px-6 py-12 text-center text-sm text-secondary"
                >
                    "没有找到匹配的路由"
                </div>
            } else {
                <div class="overflow-x-auto">
                    <table
                        class="w-full min-w-[840px] border-collapse text-left text-sm"
                    >
                        <thead
                            class="border-y border-border bg-[#fafafa] text-secondary"
                        >
                            <tr>
                                <th class="px-6 py-3 font-medium">"对外模型名"</th>
                                <th class="px-5 py-3 font-medium">
                                    "客户端 → Provider"
                                </th>
                                <th class="px-5 py-3 font-medium">"候选模型"</th>
                                <th class="px-5 py-3 font-medium">"策略"</th>
                                <th class="px-5 py-3 font-medium">"状态"</th>
                                <th class="px-6 py-3 text-right font-medium">"操作"</th>
                            </tr>
                        </thead>
                        <tbody>
                            for route in &filtered {
                                let route_id = route.id.to_string();
                                let route_version = route.version.to_string();
                                let unavailable = unavailable.clone();
                                let csrf = csrf.clone();
                                <tr
                                    class="border-b border-border last:border-b-0 hover:bg-[#fafcff]"
                                >
                                    <td class="px-6 py-2">
                                        <a
                                            class="font-mono font-medium text-heading hover:text-primary"
                                            (topcoat::runtime::link_attrs(
                                                cx, crate::app::scoped_href(cx, format!("/ui/routes/edit?id={}", route.id)),
                                                topcoat::runtime::prefetch_mode(cx),
                                            ))
                                        >
                                            (route.name.as_str())
                                        </a>
                                    </td>
                                    <td class="px-5 py-2">
                                        tag(
                                            tone: TagTone::Default,
                                            (protocol_label(route.protocol))
                                        )
                                        <span class="mx-1 text-secondary">"→"</span>
                                        tag(
                                            tone: TagTone::Default,
                                            (protocol_label(route.provider_protocol))
                                        )
                                    </td>
                                    <td class="px-5 py-2 tabular-nums">
                                        (route
                                            .targets
                                            .iter()
                                            .filter(
                                                |target| target.enabled && target.model.provider_enabled,
                                            )
                                            .count())
                                        " / "
                                        (route.targets.len())
                                    </td>
                                    <td class="px-5 py-2 text-secondary">"顺序优先"</td>
                                    <td class="px-5 py-2">
                                        tag(
                                            tone: if route_available(route) {
                                                TagTone::Success
                                            } else {
                                                TagTone::Warning
                                            },
                                            (if route_available(route) {
                                                "可用"
                                            } else if route.enabled {
                                                "无可选模型"
                                            } else {
                                                "已停用"
                                            })
                                        )
                                    </td>
                                    <td class="px-6 py-2 text-right">
                                        <div class="flex justify-end gap-3">
                                            <a
                                                class="text-primary hover:underline"
                                                (topcoat::runtime::link_attrs(
                                                    cx, crate::app::scoped_href(cx, format!("/ui/routes/edit?id={}", route.id)),
                                                    topcoat::runtime::prefetch_mode(cx),
                                                ))
                                            >
                                                "编辑"
                                            </a>
                                            <button
                                                class="border-0 bg-transparent p-0 text-[#cf1322] hover:underline"
                                                type="button"
                                                @click=$(async |_event: Event| {
                                                    let accepted = raw!(
                                                        "window.confirm('确认删除此模型路由？所有组都将移除此路由。仅需移出某个组，请到资源组管理。')",
                                                        false,
                                                    );
                                                    if !accepted {
                                                        return;
                                                    }
                                                    let result = raw!(
                                                        "await Promise.resolve(${delete_route}.call(${csrf}, ${route_id}, ${route_version})).catch(() => ${unavailable})",
                                                        unavailable.clone(),
                                                    );
                                                    if result.is_ok() {
                                                        raw!("window.location.reload()", ());
                                                    } else {
                                                        error.set(result.unwrap_err());
                                                    }
                                                })
                                            >
                                                "删除"
                                            </button>
                                        </div>
                                    </td>
                                </tr>
                            }
                        </tbody>
                    </table>
                </div>
            }
        </section>
    }.boxed())
}

fn route_available(route: &ModelRouteView) -> bool {
    route.enabled
        && route
            .targets
            .iter()
            .any(|target| target.enabled && target.model.provider_enabled)
}

#[derive(Default, Deserialize)]
pub struct EditQuery {
    id: Option<i64>,
}

#[page("./edit")]
pub async fn edit(cx: &Cx, Form(query): Form<EditQuery>) -> Result<impl View> {
    let route_list = crate::app::store(cx).list_all_routes().await?;
    let route = match query.id {
        Some(id) => Some(
            route_list
                .into_iter()
                .find(|route| route.id == id)
                .ok_or(StoreError::NotFound)?,
        ),
        None => None,
    };
    let mut models = crate::app::store(cx).list_all_models().await?;
    if let Some(route) = &route {
        models.sort_by_key(|model| {
            route
                .targets
                .iter()
                .position(|target| target.model.id == model.id)
                .unwrap_or(usize::MAX)
        });
    }
    let selected_count = route.as_ref().map_or(0, |route| route.targets.len());
    let selected_protocol = route.as_ref().map(|route| route.protocol);
    let selected_provider_protocol = route.as_ref().map(|route| route.provider_protocol);
    let busy = signal(cx, || false);
    let error = signal(cx, String::new);
    let csrf = crate::app::auth::csrf_token(cx);
    let id = route
        .as_ref()
        .map_or(String::new(), |route| route.id.to_string());
    let version = route
        .as_ref()
        .map_or(String::new(), |route| route.version.to_string());
    let title = if route.is_some() {
        "编辑模型路由"
    } else {
        "新建模型路由"
    };
    let destination = crate::app::scoped_href(cx, "/ui/routes");
    let unavailable: Outcome = Err("保存失败，请刷新后重试".into());
    Ok(view! {
        <section class=(PAGE_HEADING)>
            <div>
                <a
                    class="mb-2 inline-block text-[13px] text-secondary hover:text-primary"
                    (topcoat::runtime::link_attrs(
                        cx, crate::app::scoped_href(cx, "/ui/routes"),
                        topcoat::runtime::prefetch_mode(cx),
                    ))
                >
                    "← 返回 Model Routes"
                </a>
                <h1 class="mb-2 mt-0 text-base font-semibold">(title)</h1>
                <p>
                    "路由名称是客户端请求中的 model；客户端与 Provider 协议可分别选择。跨协议目前只支持非流式 JSON。"
                </p>
            </div>
        </section>
        <form
            id="route-editor-form"
            class="max-w-[1100px] overflow-hidden rounded-lg border border-border bg-white shadow-xs"
            @submit=$(async |event: Event| {
                event.prevent_default();
                if busy.get() {
                    return;
                }
                busy.set(true);
                error.set("".to_owned());
                let result = raw!(
                    "await Promise.resolve((() => { const form = new FormData(document.getElementById('route-editor-form')); const targets = form.getAll('model').map((value, index) => ({model_id: Number(value), position: Number(form.get('position-' + value)) || 9999, index})).sort((a, b) => a.position - b.position || a.index - b.index).map(({model_id}) => ({model_id, enabled: true})); return ${save_route}.call(${csrf}, ${id}, ${version}, cx.hydrate(String(form.get('name') || '')), cx.hydrate(String(form.get('protocol') || '')), cx.hydrate(String(form.get('provider_protocol') || '')), cx.hydrate(form.has('enabled')), cx.hydrate(JSON.stringify(targets))); })()).catch(() => ${unavailable})",
                    unavailable.clone(),
                );
                busy.set(false);
                if result.is_ok() {
                    raw!("window.location.assign(${destination}.dehydrate())", ());
                } else {
                    error.set(result.unwrap_err());
                }
            })
        >
            <div class="grid gap-5 border-b border-border p-6 max-[640px]:p-4">
                <div
                    class="grid grid-cols-[minmax(0,1fr)_auto] items-end gap-5 max-[640px]:grid-cols-1"
                >
                    <div>
                        <label class="mb-2 block text-sm font-medium" for="route-name">
                            "路由名称 · 对外模型名"
                        </label>
                        <input
                            id="route-name"
                            class="h-10 w-full rounded-md border border-control-border px-3 font-mono text-sm focus:border-primary"
                            name="name"
                            maxlength="200"
                            required=(true)
                            value=(route
                                .as_ref()
                                .map_or("", |route| route.name.as_str()))
                            placeholder="例如 smart-chat"
                        >
                    </div>
                    <label class="flex h-10 items-center gap-2 text-sm">
                        <input
                            type="checkbox"
                            name="enabled"
                            checked=(route.as_ref().is_none_or(|route| route.enabled))
                        >
                        "启用路由"
                    </label>
                </div>
                <div>
                    <label class="mb-2 block text-sm font-medium" for="route-protocol">
                        "客户端协议"
                    </label>
                    <select
                        id="route-protocol"
                        class="h-10 w-full max-w-[360px] rounded-md border border-control-border bg-white px-3 text-sm focus:border-primary"
                        name="protocol"
                        required=(true)
                    >
                        <option
                            value=""
                            selected=(selected_protocol.is_none())
                            disabled=(true)
                        >
                            "请选择协议"
                        </option>
                        for protocol in PROTOCOLS {
                            <option
                                value=(protocol.as_str())
                                selected=(selected_protocol == Some(protocol))
                            >
                                (protocol_label(protocol))
                            </option>
                        }
                    </select>
                </div>
                <div>
                    <label
                        class="mb-2 block text-sm font-medium"
                        for="route-provider-protocol"
                    >
                        "Provider 协议"
                    </label>
                    <select
                        id="route-provider-protocol"
                        class="h-10 w-full max-w-[360px] rounded-md border border-control-border bg-white px-3 text-sm focus:border-primary"
                        name="provider_protocol"
                        required=(true)
                        @change=$(|_event: Event| {
                            raw!(
                                "{ const protocol = ${_event}.target.value.dehydrate(); const query = (document.querySelector('#route-model-picker input[type=search]')?.value || '').trim().toLowerCase(); for (const option of document.querySelectorAll('#route-model-options [data-search]')) { const supported = option.dataset.protocols.includes(',' + protocol + ','); const box = option.querySelector('input[name=model]'); if (!supported && box.checked) box.click(); option.hidden = !supported || !option.dataset.search.toLowerCase().includes(query); option.style.display = option.hidden ? 'none' : ''; } }",
                                (),
                            );
                        })
                    >
                        <option
                            value=""
                            selected=(selected_provider_protocol.is_none())
                            disabled=(true)
                        >
                            "请选择协议"
                        </option>
                        for protocol in PROTOCOLS {
                            <option
                                value=(protocol.as_str())
                                selected=(selected_provider_protocol == Some(protocol))
                            >
                                (protocol_label(protocol))
                            </option>
                        }
                    </select>
                </div>
                <div
                    class="rounded-md border border-[#d6e4ff] bg-[#f5f9ff] px-4 py-3 text-[13px] text-[#24539a]"
                >
                    "策略：顺序优先。停用的 Provider 会被跳过；跨协议仅支持非流式 JSON，请求选定目标后不自动重试。"
                </div>
                <p
                    class="m-0 text-sm text-[#cf1322]"
                    role="alert"
                    :hidden=$(error.get().is_empty())
                >
                    $(error.get())
                </p>
            </div>
            <div class="px-6 py-5 max-[640px]:px-4">
                <h2 class="m-0 text-base font-semibold">"候选模型"</h2>
                <p class="mt-1 mb-4 text-[13px] text-secondary">
                    "在下拉框中搜索并多选模型；下方仅显示已选模型，可调整顺序或删除。"
                </p>
                if models.is_empty() {
                    <div
                        class="rounded-md border border-dashed border-border p-8 text-center text-sm text-secondary"
                    >
                        "尚无可选模型。请先到 Models 添加上游模型。"
                    </div>
                } else {
                    <details
                        id="route-model-picker"
                        class="mb-5 w-full max-w-[560px] rounded-md border border-control-border bg-white shadow-xs"
                    >
                        <summary
                            class="flex h-10 cursor-pointer list-none items-center justify-between gap-3 px-3 text-sm text-heading [&::-webkit-details-marker]:hidden"
                        >
                            <span>"搜索并选择模型"</span>
                            <span class="flex items-center gap-2 text-secondary">
                                "已选 "
                                <span
                                    id="route-selected-count"
                                    class="font-medium text-primary"
                                >
                                    (selected_count)
                                </span>
                                " 个"
                                <span aria-hidden="true">"⌄"</span>
                            </span>
                        </summary>
                        <div class="border-t border-border p-2">
                            <input
                                class="mb-2 h-9 w-full rounded-md border border-control-border px-3 text-sm focus:border-primary"
                                type="search"
                                placeholder="搜索模型标识、上游 ID 或 Provider"
                                aria-label="搜索候选模型"
                                @input=$(|_event: Event| {
                                    raw!(
                                        "{ const query = ${_event}.target.value.dehydrate().trim().toLowerCase(); const protocol = document.getElementById('route-provider-protocol').value; for (const option of document.querySelectorAll('#route-model-options [data-search]')) { option.hidden = !option.dataset.protocols.includes(',' + protocol + ',') || !option.dataset.search.toLowerCase().includes(query); option.style.display = option.hidden ? 'none' : ''; } }",
                                        (),
                                    );
                                })
                            >
                            <div
                                id="route-model-options"
                                class="max-h-[280px] overflow-y-auto"
                            >
                                for model in &models {
                                    let existing = route
                                        .as_ref()
                                        .and_then(
                                            |route| route
                                                    .targets
                                                    .iter()
                                                    .find(|target| target.model.id == model.id),
                                        );
                                    let search_text = format!(
                                        "{} {} {}",
                                        model.alias,
                                        model.upstream_model_id,
                                        model.provider_name,
                                    );
                                    let protocols = format!(
                                        ",{},",
                                        model
                                            .protocols
                                            .iter()
                                            .map(|protocol| protocol.as_str())
                                            .collect::<Vec<_>>()
                                            .join(","),
                                    );
                                    <label
                                        class="flex cursor-pointer items-start gap-3 rounded px-2 py-2 text-sm hover:bg-primary-soft"
                                        data-search=(search_text)
                                        data-protocols=(protocols)
                                        hidden=(!selected_provider_protocol.is_some_and(
                                            |protocol| model.protocols.contains(&protocol),
                                        ))
                                        style=(if selected_provider_protocol.is_some_and(
                                            |protocol| model.protocols.contains(&protocol),
                                        ) {
                                            ""
                                        } else {
                                            "display:none"
                                        })
                                    >
                                        <input
                                            class="mt-1"
                                            type="checkbox"
                                            name="model"
                                            value=(model.id)
                                            checked=(existing.is_some())
                                            disabled=(!model.provider_enabled && existing.is_none())
                                            data-provider-enabled=(if model.provider_enabled {
                                                "true"
                                            } else {
                                                "false"
                                            })
                                            @change=$(|_event: Event| {
                                                raw!(
                                                    "{ const modelId = ${_event}.target.value.dehydrate(); const box = document.querySelector('#route-model-options input[name=model][value=\"' + modelId + '\"]'); const row = document.getElementById('route-target-' + modelId); if (box.checked) { const values = [...document.querySelectorAll('#route-model-options input[name=model]:checked')].filter(input => input.value !== modelId).map(input => Number(document.getElementById('route-target-' + input.value).querySelector('input[type=number]').value) || 0); row.querySelector('input[type=number]').value = Math.max(0, ...values) + 1; } if (!box.checked && box.dataset.providerEnabled === 'false') box.disabled = true; const tbody = document.querySelector('#route-selected-results tbody'); for (const target of tbody.querySelectorAll('tr')) { const id = target.id.slice('route-target-'.length); const selected = document.querySelector('#route-model-options input[name=model][value=\"' + id + '\"]').checked; target.hidden = !selected; target.style.display = selected ? '' : 'none'; target.querySelector('input[type=number]').disabled = !selected; } const active = [...tbody.querySelectorAll('tr')].filter(target => !target.hidden).sort((a, b) => Number(a.querySelector('input[type=number]').value) - Number(b.querySelector('input[type=number]').value)); active.forEach((target, index) => { target.querySelector('input[type=number]').value = index + 1; tbody.appendChild(target); }); const count = active.length; document.getElementById('route-selected-count').textContent = count; document.getElementById('route-selected-empty').style.display = count ? 'none' : ''; document.getElementById('route-selected-results').style.display = count ? '' : 'none'; }",
                                                    (),
                                                );
                                            })
                                            aria-label=(format!("选择 {}", model.alias))
                                        >
                                        <span class="min-w-0">
                                            <span class="block truncate font-medium text-heading">
                                                (model.alias.as_str())
                                            </span>
                                            <span
                                                class="block truncate font-mono text-xs text-secondary"
                                            >
                                                (model.provider_name.as_str())
                                                " · "
                                                (model.upstream_model_id.as_str())
                                                if !model.provider_enabled {
                                                    <span class="text-[#d48806]">
                                                        " · Provider 已停用"
                                                    </span>
                                                }
                                            </span>
                                        </span>
                                    </label>
                                }
                            </div>
                        </div>
                    </details>
                    <p
                        id="route-selected-empty"
                        class="rounded-md border border-dashed border-border p-8 text-center text-sm text-secondary"
                        style=(if selected_count > 0 { "display:none" } else { "" })
                    >
                        "尚未选择候选模型。"
                    </p>
                    <div
                        id="route-selected-results"
                        class="overflow-x-auto rounded-md border border-border"
                        style=(if selected_count == 0 { "display:none" } else { "" })
                    >
                        <table
                            class="w-full min-w-[700px] border-collapse text-left text-sm"
                        >
                            <thead class="bg-[#fafafa] text-secondary">
                                <tr>
                                    <th class="w-24 px-4 py-3 font-medium">"顺序"</th>
                                    <th class="px-4 py-3 font-medium">"具体模型"</th>
                                    <th class="px-4 py-3 font-medium">"Provider"</th>
                                    <th class="px-4 py-3 font-medium">"协议"</th>
                                    <th class="w-24 px-4 py-3 font-medium">"操作"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for (index, model) in models.iter().enumerate() {
                                    let existing = route
                                        .as_ref()
                                        .and_then(
                                            |route| route
                                                    .targets
                                                    .iter()
                                                    .enumerate()
                                                    .find(|(_, target)| target.model.id == model.id),
                                        );
                                    let selected = existing.is_some();
                                    let model_id = model.id.to_string();
                                    <tr
                                        id=(format!("route-target-{}", model.id))
                                        class="border-t border-border"
                                        hidden=(!selected)
                                    >
                                        <td class="px-4 py-2">
                                            <input
                                                class="h-8 w-16 rounded border border-control-border px-2 text-sm tabular-nums"
                                                type="number"
                                                min="1"
                                                max="9999"
                                                name=(format!("position-{}", model.id))
                                                value=(existing.map_or(
                                                    index + 1,
                                                    |(position, _)| position + 1,
                                                ))
                                                disabled=(!selected)
                                                aria-label=(format!("{} 的顺序", model.alias))
                                                @change=$(|_event: Event| {
                                                    raw!(
                                                        "{ const current = document.getElementById('route-target-' + ${_event}.target.name.dehydrate().slice(9)); const tbody = current.parentElement; const rows = [...tbody.querySelectorAll('tr')].filter(row => !row.hidden && row !== current).sort((a, b) => Number(a.querySelector('input[type=number]').value) - Number(b.querySelector('input[type=number]').value)); const position = Math.max(1, Math.min(rows.length + 1, Number(${_event}.target.value.dehydrate()) || 1)); rows.splice(position - 1, 0, current); rows.forEach((row, index) => { row.querySelector('input[type=number]').value = index + 1; tbody.appendChild(row); }); }",
                                                        (),
                                                    );
                                                })
                                            >
                                        </td>
                                        <td class="px-4 py-2">
                                            <span class="font-medium">(model.alias.as_str())</span>
                                            <span class="mt-1 block font-mono text-xs text-secondary">
                                                (model.upstream_model_id.as_str())
                                            </span>
                                        </td>
                                        <td class="px-4 py-2">
                                            (model.provider_name.as_str())
                                            if !model.provider_enabled {
                                                <span class="block text-xs text-[#d48806]">
                                                    "已停用"
                                                </span>
                                            }
                                        </td>
                                        <td class="px-4 py-2">
                                            <div class="flex flex-wrap gap-1">
                                                for protocol in &model.protocols {
                                                    tag(tone: TagTone::Default, (protocol_label(*protocol)))
                                                }
                                            </div>
                                        </td>
                                        <td class="px-4 py-2">
                                            <button
                                                class="border-0 bg-transparent p-0 text-[#cf1322] hover:underline"
                                                type="button"
                                                @click=$(|_event: Event| {
                                                    raw!(
                                                        "document.querySelector('#route-model-options input[name=model][value=\"' + ${model_id}.dehydrate() + '\"]').click()",
                                                        (),
                                                    );
                                                })
                                                aria-label=(format!("删除 {}", model.alias))
                                            >
                                                "删除"
                                            </button>
                                        </td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                    </div>
                }
            </div>
            <footer
                class="flex justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4"
            >
                <a
                    class=(BUTTON)
                    (topcoat::runtime::link_attrs(
                        cx, crate::app::scoped_href(cx, "/ui/routes"),
                        topcoat::runtime::prefetch_mode(cx),
                    ))
                >
                    "取消"
                </a>
                <button
                    class=(class!(BUTTON, PRIMARY))
                    type="submit"
                    :disabled=$(busy.get())
                >
                    $(if busy.get() { "保存中…" } else { "保存路由" })
                </button>
            </footer>
        </form>
    })
}

#[derive(Deserialize)]
struct TargetForm {
    model_id: i64,
    enabled: bool,
}

#[procedure("/ui/_topcoat/runtime/procedures/save-route")]
#[expect(
    clippy::too_many_arguments,
    reason = "Topcoat 按表单字段逐个注入过程参数"
)]
pub async fn save_route(
    cx: &Cx,
    csrf: String,
    id: String,
    version: String,
    name: String,
    protocol: String,
    provider_protocol: String,
    enabled: bool,
    targets_json: String,
) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let targets: Vec<TargetForm> = serde_json::from_str(&targets_json)
        .map_err(|_| topcoat::router::error::bad_request("无效的候选模型列表"))?;
    let protocol = match protocol.as_str() {
        "openai_chat" => Protocol::OpenAiChat,
        "openai_responses" => Protocol::OpenAiResponses,
        "anthropic_messages" => Protocol::AnthropicMessages,
        "gemini" => Protocol::Gemini,
        _ => return Ok(Err("请选择路由协议".into())),
    };
    let provider_protocol = match provider_protocol.as_str() {
        "openai_chat" => Protocol::OpenAiChat,
        "openai_responses" => Protocol::OpenAiResponses,
        "anthropic_messages" => Protocol::AnthropicMessages,
        "gemini" => Protocol::Gemini,
        _ => return Ok(Err("请选择 Provider 协议".into())),
    };
    let input = ModelRouteInput {
        name,
        protocol,
        provider_protocol,
        enabled,
        targets: targets
            .into_iter()
            .map(|target| ModelRouteTargetInput {
                model_id: target.model_id,
                enabled: target.enabled,
            })
            .collect(),
    };
    let store = &crate::app::store(cx);
    let result = if id.is_empty() {
        store.create_route(input).await
    } else {
        match (id.parse::<i64>(), version.parse::<u64>()) {
            (Ok(id), Ok(version)) => store.update_route(id, version, input).await,
            _ => Err(StoreError::Validation("路由版本无效，请刷新后重试".into())),
        }
    };
    Ok(result
        .map(|route| format!("「{}」已保存", route.name))
        .map_err(|error| error.to_string()))
}

#[procedure("/ui/_topcoat/runtime/procedures/delete-route")]
pub async fn delete_route(cx: &Cx, csrf: String, id: String, version: String) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let result = match (id.parse::<i64>(), version.parse::<u64>()) {
        (Ok(id), Ok(version)) => crate::app::store(cx).delete_route(id, version).await,
        _ => Err(StoreError::Validation("路由版本无效，请刷新后重试".into())),
    };
    Ok(result
        .map(|_| "模型路由已删除".to_owned())
        .map_err(|error| error.to_string()))
}
