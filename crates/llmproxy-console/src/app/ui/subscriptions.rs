pub(crate) mod ownership;

use crate::app::{AppState, check_csrf};
use llmproxy_core::subscription::Health;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{bad_request, see_other},
        page, route,
    },
    runtime::{Event, procedure, signal},
    view::{View, view},
};

use topcoat_ant_design::{UiLanguage, popconfirm, popconfirm_trigger_attributes};
const TEXT_LINK: &str =
    "border-0 bg-transparent p-0 text-sm whitespace-nowrap text-primary hover:text-primary-hover";

#[derive(Deserialize)]
pub struct NodeQuery {
    error: Option<String>,
}

#[page]
pub async fn nodes(cx: &Cx, Form(query): Form<NodeQuery>) -> Result<impl View> {
    let refresh = signal(cx, || 0.0);
    let _revision = refresh.get();
    let claim_form = super::forms::FormState::new(cx, Some(&refresh));
    let personal = crate::app::store(cx).is_personal();
    let state = app_context::<AppState>(cx);
    let nodes = crate::app::store(cx)
        .subscription_nodes()
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    let providers = crate::app::store(cx).list().await?;
    let presence = state.subscriptions.lock().unwrap().clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let configured = std::env::var("LLMPROXY_SUBSCRIPTION_REGISTRATION_KEY").is_ok();
    let online = nodes
        .iter()
        .filter(|node| {
            presence
                .get(&node.node_id)
                .is_some_and(|entry| entry.online_until > now)
        })
        .count();
    let csrf = crate::app::auth::csrf_token(cx);
    Ok(view! {
        super::forms::feedback(state: &claim_form)
        if let Some(error) = &query.error {
            <p
                class="rounded border border-red-200 bg-red-50 p-4 text-red-700"
                role="alert"
            >
                (error)
            </p>
        }
        <div class=(super::providers::PAGE_HEADING)>
            <div>
                <h1 class="sr-only">"订阅节点"</h1>
                <p>
                    "查看个人订阅代理的连接与后端状态，并决定是否提供代理服务。"
                </p>
            </div>
            <div class="flex flex-wrap items-center gap-3">
                if personal {
                    ownership::claim_editor(state: &claim_form, csrf: csrf.as_str())
                }
                <a
                    class=(super::providers::BUTTON)
                    (topcoat::runtime::link_attrs(
                        cx,
                        "/ui/subscriptions",
                        topcoat::runtime::PrefetchMode::Never,
                    ))
                >
                    "刷新状态"
                </a>
            </div>
        </div>
        <div
            class="mb-6 flex items-center gap-6 rounded-lg border border-border bg-white px-5 py-4 text-sm"
        >
            <span>
                "已登记 "
                <strong>(nodes.len())</strong>
            </span>
            <span>
                "在线 "
                <strong>(online)</strong>
            </span>
            <span class="text-secondary">
                "首次登记默认停用。启用后在 Models 和 Model Routes 中配置模型与路由。"
            </span>
        </div>
        if !configured {
            <p class="rounded-lg border border-border bg-white p-5 text-secondary">
                "远程节点接入尚未配置。"
            </p>
        }
        <div class="overflow-x-auto rounded-lg border border-border bg-white">
            <table
                class="w-full border-collapse text-left text-sm [&_th]:whitespace-nowrap [&_th]:border-b [&_th]:border-border [&_th]:bg-surface [&_th]:px-5 [&_th]:py-3 [&_th]:font-medium [&_td]:border-b [&_td]:border-border [&_td]:px-5 [&_td]:py-2"
            >
                <thead>
                    <tr>
                        <th>"节点"</th>
                        <th>"别名"</th>
                        <th>"后端"</th>
                        <th>"连接"</th>
                        <th>"后端状态"</th>
                        <th>"服务准入"</th>
                        <th>"Provider"</th>
                        <th>"操作"</th>
                    </tr>
                </thead>
                <tbody>
                    #[key(node.node_id.clone())]
                    for node in &nodes {
                        <tr>
                            <td>
                                <strong>(node.name.clone())</strong>
                                <p
                                    class="m-0 mt-1 text-xs text-muted"
                                    title=(node.node_id.clone())
                                >
                                    (format!(
                                        "{} · {} 个模型 · 并发 {}",
                                        node.node_id.get(..12).unwrap_or(&node.node_id),
                                        node.models.len(),
                                        node.concurrency,
                                    ))
                                </p>
                            </td>
                            <td>node_alias(node: node, csrf: csrf.as_str())</td>
                            <td>(node.backend.clone())</td>
                            <td>
                                (if presence
                                    .get(&node.node_id)
                                    .is_some_and(|entry| entry.online_until > now) {
                                    "在线"
                                } else {
                                    "离线"
                                })
                            </td>
                            <td>
                                (match presence
                                    .get(&node.node_id)
                                    .filter(|entry| entry.online_until > now)
                                    .map(|entry| &entry.health) {
                                    Some(Health::Ready) => "就绪",
                                    Some(Health::Abnormal) => "异常",
                                    _ => "未知",
                                })
                            </td>
                            <td>
                                (if node.enabled { "允许加入" } else { "停用" })
                            </td>
                            <td>
                                (node
                                    .provider_id
                                    .and_then(
                                        |id| providers.iter().find(|provider| provider.id == id),
                                    )
                                    .map(|provider| provider.name.clone())
                                    .unwrap_or_else(|| "未关联".into()))
                            </td>
                            <td class="whitespace-nowrap [&>a]:ml-4">
                                node_action(
                                    node: node,
                                    csrf: csrf.as_str(),
                                    configured: configured
                                )
                                if node.enabled {
                                    <a
                                        class=(TEXT_LINK)
                                        href=(format!(
                                            "/ui/subscriptions/models?node_id={}",
                                            node.node_id,
                                        ))
                                    >
                                        "导入模型"
                                    </a>
                                }
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
            if nodes.is_empty() {
                <p class="m-0 px-5 py-12 text-center text-secondary">
                    (if personal {
                        "尚无关联节点。启动订阅代理连接此系统后，点击「关联节点」添加自己的节点。"
                    } else {
                        "尚无登记节点。启动本地订阅代理并连接此 llmproxy 后，节点会显示在这里。"
                    })
                </p>
            }
        </div>
    })
}

#[derive(Deserialize)]
pub struct Enable {
    csrf: String,
    node_id: String,
    version: u64,
    enabled: bool,
}

#[route(POST "/ui/subscriptions/enabled")]
pub async fn set_enabled(
    cx: &Cx,
    Form(input): Form<Enable>,
) -> Result<topcoat::router::error::SeeOther> {
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let url = url::Url::parse(&state.gateway_origin).map_err(|_| bad_request("转接地址无效"))?;
    let key = std::env::var("LLMPROXY_SUBSCRIPTION_RELAY_KEY")
        .map_err(|_| bad_request("未配置转接服务"))?;
    let target = llmproxy_store::SubscriptionRelayTarget {
        host: url.host_str().unwrap_or("").to_owned(),
        port: state.port,
        key,
    };
    let result = crate::app::store(cx)
        .set_subscription_enabled(&input.node_id, input.version, input.enabled, &target)
        .await;
    if let Err(error) = result {
        return Ok(node_error(error));
    }
    if input.enabled {
        Ok(see_other(format!(
            "/ui/subscriptions/models?node_id={}",
            input.node_id
        )))
    } else {
        Ok(see_other("/ui/subscriptions"))
    }
}

#[derive(Deserialize)]
pub struct Rename {
    csrf: String,
    node_id: String,
    version: u64,
    name: String,
}

#[route(POST "/ui/subscriptions/name")]
pub async fn rename(
    cx: &Cx,
    Form(input): Form<Rename>,
) -> Result<topcoat::router::error::SeeOther> {
    check_csrf(cx, &input.csrf)?;
    let result = crate::app::store(cx)
        .rename_subscription(&input.node_id, input.version, &input.name)
        .await;
    if let Err(error) = result {
        return Ok(node_error(error));
    }
    Ok(see_other("/ui/subscriptions"))
}

#[derive(Deserialize)]
pub struct ModelQuery {
    node_id: String,
}

async fn import_provider(cx: &Cx, node_id: &str) -> Result<i64> {
    let node = crate::app::store(cx)
        .subscription_nodes()
        .await?
        .into_iter()
        .find(|node| node.node_id == node_id && node.enabled)
        .ok_or_else(|| bad_request("请先允许节点加入服务"))?;
    node.provider_id
        .ok_or_else(|| bad_request("节点尚未关联 Provider").into())
}

#[page("/ui/subscriptions/models")]
pub async fn import_models(cx: &Cx, Form(query): Form<ModelQuery>) -> Result<impl View> {
    let id = import_provider(cx, &query.node_id).await?;
    let provider = crate::app::store(cx).get(id).await?;
    let target = crate::app::store(cx).probe_enabled_target(id).await?;
    let candidates = crate::app::model_catalog::query_models(target)
        .await
        .map_err(bad_request)?;
    let model_store = crate::app::store(cx);
    let existing = model_store.list_all_models().await?;
    let busy = signal(cx, || false);
    let error = signal(cx, String::new);
    let saved = signal(cx, || false);
    let unavailable: std::result::Result<String, String> =
        Err("导入结果未确认，请检查 Models 列表后重试".into());
    Ok(view! {
        <div class=(super::providers::PAGE_HEADING)>
            <h1 class="m-0 text-base font-semibold">"导入订阅模型"</h1>
        </div>
        <p>
            (provider.name)
            "：选择需要导入系统的模型，再到资源组中添加模型或路由。"
        </p>
        <form
            class="rounded-lg border border-border bg-white p-5"
            id="subscription-model-import"
            method="post"
            action="/ui/subscriptions/models"
            @submit=$(async |event: Event| {
                event.prevent_default();
                if busy.get() { return; }
                busy.set(true);
                error.set("".to_owned());
                let _payload = raw!("cx.hydrate(new URLSearchParams(new FormData(document.getElementById('subscription-model-import'))).toString())", String::new());
                let result = raw!("await Promise.resolve(${save_import}.call(${_payload})).catch(() => ${unavailable})", unavailable.clone());
                busy.set(false);
                if result.is_ok() {
                    saved.set(true);
                } else {
                    error.set(result.unwrap_err());
                }
            })
        >
            <input type="hidden" name="csrf" value=(crate::app::auth::csrf_token(cx))>
            <input type="hidden" name="node_id" value=(query.node_id.clone())>
            <p role="alert" class="text-danger" :hidden=$(error.get().is_empty())>$(error.get())</p>
            <div :hidden=$(saved.get())>
            for candidate in &candidates {
                <label class="block my-3">
                    <input
                        type="checkbox"
                        name="models"
                        value=(candidate.id.clone())
                        disabled=(existing
                            .iter()
                            .any(
                                |model| model.provider_id == id
                                        && model.upstream_model_id == candidate.id,
                            ))
                    >
                    (candidate.id.clone())
                    if existing
                        .iter()
                        .any(
                            |model| model.provider_id == id
                                    && model.upstream_model_id == candidate.id,
                        ) {
                        "（已导入）"
                    }
                </label>
            }
            <button class=(super::providers::BUTTON) type="submit" :disabled=$(busy.get())>
                "导入所选模型"
            </button>
            </div>
            <p role="status" :hidden=$(!saved.get())>"已导入系统，尚未加入任何资源组。"<a class="ml-2 text-primary" href="/ui/groups">"前往资源组添加"</a><a class="ml-2 text-primary" href="/ui/models">"查看模型列表"</a></p>
            <a class=(super::providers::BUTTON) href="/ui/subscriptions">"返回"</a>
        </form>
    })
}

pub struct Import {
    csrf: String,
    node_id: String,
    models: Vec<String>,
}

#[procedure("/ui/_topcoat/runtime/procedures/import-subscription-models")]
pub async fn save_import(cx: &Cx, payload: String) -> Result<std::result::Result<String, String>> {
    let Form(fields) = Form::<Vec<(String, String)>>::from_bytes(payload.as_bytes())?;
    let input = import_fields(fields);
    check_csrf(cx, &input.csrf)?;
    Ok(import_selected_models(cx, input)
        .await
        .map(|_| "模型已导入系统".to_owned())
        .map_err(|error| error.to_string()))
}

async fn import_selected_models(cx: &Cx, input: Import) -> Result<()> {
    let id = import_provider(cx, &input.node_id).await?;
    let provider = crate::app::store(cx).get(id).await?;
    let candidates = crate::app::model_catalog::query_models(
        crate::app::store(cx).probe_enabled_target(id).await?,
    )
    .await
    .map_err(bad_request)?;
    if input.models.is_empty()
        || input
            .models
            .iter()
            .any(|model| !candidates.iter().any(|candidate| &candidate.id == model))
    {
        return Err(bad_request("请选择节点提供的模型").into());
    }
    let mut selected = input.models;
    selected.sort();
    selected.dedup();
    let existing = crate::app::store(cx).list_all_models().await?;
    let mappings = selected
        .into_iter()
        .filter(|model| {
            !existing
                .iter()
                .any(|old| old.provider_id == id && old.upstream_model_id == *model)
        })
        .map(|model| llmproxy_store::ModelMappingInput {
            alias: format!("{}/{}", provider.name, model),
            provider_id: id,
            upstream_model_id: model,
            protocols: vec![llmproxy_core::protocol::Protocol::OpenAiResponses],
            reference_price: None,
            thinking: Default::default(),
        })
        .collect::<Vec<_>>();
    if !mappings.is_empty() {
        crate::app::store(cx).create_models(mappings).await?;
    }
    Ok(())
}

fn import_fields(fields: Vec<(String, String)>) -> Import {
    let mut input = Import {
        csrf: String::new(),
        node_id: String::new(),
        models: Vec::new(),
    };
    for (key, value) in fields {
        match key.as_str() {
            "csrf" => input.csrf = value,
            "node_id" => input.node_id = value,
            "models" => input.models.push(value),
            _ => {}
        }
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_form_accepts_multiple_models_and_empty_selection() {
        let Form(fields) = Form::<Vec<(String, String)>>::from_bytes(
            b"csrf=test&node_id=node&models=model-a&models=model-b",
        )
        .unwrap();
        assert_eq!(import_fields(fields).models, ["model-a", "model-b"]);
        let Form(fields) =
            Form::<Vec<(String, String)>>::from_bytes(b"csrf=test&node_id=node").unwrap();
        assert!(import_fields(fields).models.is_empty());
    }
}

fn node_error(error: llmproxy_store::StoreError) -> topcoat::router::error::SeeOther {
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("error", &error.to_string())
        .finish();
    see_other(format!("/ui/subscriptions?{query}"))
}

#[topcoat::view::component]
async fn node_action(
    cx: &Cx,
    node: &llmproxy_store::SubscriptionNodeView,
    csrf: &str,
    configured: bool,
) -> Result<impl View> {
    let id = format!("disable-node-{}", node.node_id);
    let trigger = popconfirm_trigger_attributes(cx, &id);
    let title = format!("确认停用「{}」？", node.name);
    Ok(view! {
        if node.enabled {
            <button class=(TEXT_LINK) type="button" (trigger)>"停用"</button>
            popconfirm(
                id: id.as_str(),
                title: title.as_str(),
                language: UiLanguage::ChineseSimplified,
                <form method="post" action="/ui/subscriptions/enabled">
                    <input type="hidden" name="csrf" value=(csrf)>
                    <input type="hidden" name="node_id" value=(&node.node_id)>
                    <input type="hidden" name="version" value=(node.version)>
                    <input type="hidden" name="enabled" value="false">
                    <button class="gr-button gr-button-danger" type="submit">
                        "确认停用"
                    </button>
                </form>
            )
        } else {
            <form class="inline-block" method="post" action="/ui/subscriptions/enabled">
                <input type="hidden" name="csrf" value=(csrf)>
                <input type="hidden" name="node_id" value=(&node.node_id)>
                <input type="hidden" name="version" value=(node.version)>
                <input type="hidden" name="enabled" value="true">
                <button class=(TEXT_LINK) type="submit" disabled=(!configured)>
                    "允许加入"
                </button>
            </form>
        }
    })
}

#[topcoat::view::component]
async fn node_alias(
    cx: &Cx,
    node: &llmproxy_store::SubscriptionNodeView,
    csrf: &str,
) -> Result<impl View> {
    let editing = signal(cx, || false);
    let original = node.provider_name.clone().unwrap_or_default();
    let draft = signal(cx, || original.clone());
    Ok(view! {
        <div
            class="flex items-center gap-3 [&[hidden]]:hidden"
            :hidden=$(editing.get())
        >
            <span class="max-w-48 truncate" title=(original.clone())>
                (if original.is_empty() { "—" } else { original.as_str() })
            </span>
            <button
                class=(TEXT_LINK)
                type="button"
                @click=$(|_event: Event| {
                    draft.set(original.clone());
                    editing.set(true);
                })
            >
                "编辑"
            </button>
        </div>
        <form
            class="flex items-center gap-3 [&[hidden]]:hidden"
            :hidden=$(!editing.get())
            method="post"
            action="/ui/subscriptions/name"
        >
            <input type="hidden" name="csrf" value=(csrf)>
            <input type="hidden" name="node_id" value=(&node.node_id)>
            <input type="hidden" name="version" value=(node.version)>
            <input
                class="h-8 w-48 rounded border border-border px-2 focus:border-primary focus:outline-none"
                name="name"
                :value=$(draft.get())
                @input=$(|event: Event| draft.set(event.target.value))
                placeholder="别名（可选）"
                maxlength="128"
                aria-label="Provider 别名"
            >
            <button class=(TEXT_LINK) type="submit">"保存"</button>
            <button
                class=(TEXT_LINK)
                type="button"
                @click=$(|_event: Event| {
                    draft.set(original.clone());
                    editing.set(false);
                })
            >
                "取消"
            </button>
        </form>
    })
}
