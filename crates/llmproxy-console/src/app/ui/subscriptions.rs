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
    view::{View, view},
};

#[page]
pub async fn nodes(cx: &Cx) -> Result<impl View> {
    let state = app_context::<AppState>(cx);
    let nodes = state
        .store
        .subscription_nodes()
        .await
        .map_err(|error| bad_request(error.to_string()))?;
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
    Ok(view! {
        <div class=(super::providers::PAGE_HEADING)>
            <div>
                <h1>"订阅节点"</h1>
                <p>
                    "查看个人订阅代理的连接与后端状态，并决定是否提供代理服务。"
                </p>
            </div>
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
                class="w-full border-collapse text-left text-sm [&_th]:whitespace-nowrap [&_th]:border-b [&_th]:border-border [&_th]:bg-surface [&_th]:px-5 [&_th]:py-3 [&_th]:font-medium [&_td]:border-b [&_td]:border-border [&_td]:px-5 [&_td]:py-4"
            >
                <thead>
                    <tr>
                        <th>"节点"</th>
                        <th>"后端"</th>
                        <th>"连接"</th>
                        <th>"后端状态"</th>
                        <th>"服务准入"</th>
                        <th>"Provider"</th>
                        <th>"操作"</th>
                    </tr>
                </thead>
                <tbody>
                    for node in &nodes {
                        <tr>
                            <td>
                                <form method="post" action="/ui/subscriptions/name">
                                    <input
                                        type="hidden"
                                        name="csrf"
                                        value=(state.csrf.clone())
                                    >
                                    <input
                                        type="hidden"
                                        name="node_id"
                                        value=(node.node_id.clone())
                                    >
                                    <input
                                        type="hidden"
                                        name="version"
                                        value=(node.version.to_string())
                                    >
                                    <input
                                        class="rounded border border-border px-2 py-1 mr-2"
                                        name="name"
                                        value=(node.name.clone())
                                        required=(true)
                                        maxlength="128"
                                        aria-label="节点名称"
                                    >
                                    <button class=(super::providers::BUTTON) type="submit">
                                        "保存名称"
                                    </button>
                                </form>
                                <p
                                    class="mt-1 text-xs text-muted"
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
                                    .map(|id| id.to_string())
                                    .unwrap_or_else(|| "未关联".into()))
                            </td>
                            <td>
                                <form method="post" action="/ui/subscriptions/enabled">
                                    <input
                                        type="hidden"
                                        name="csrf"
                                        value=(state.csrf.clone())
                                    >
                                    <input
                                        type="hidden"
                                        name="node_id"
                                        value=(node.node_id.clone())
                                    >
                                    <input
                                        type="hidden"
                                        name="version"
                                        value=(node.version.to_string())
                                    >
                                    <input
                                        type="hidden"
                                        name="enabled"
                                        value=((!node.enabled).to_string())
                                    >
                                    <button
                                        class=(super::providers::BUTTON)
                                        type="submit"
                                        disabled=(!configured)
                                    >
                                        (if node.enabled { "停用" } else { "允许加入" })
                                    </button>
                                </form>
                                if node.enabled {
                                    <a
                                        class=(super::providers::BUTTON)
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
                    "尚无登记节点。启动本地订阅代理并连接此 llmproxy 后，节点会显示在这里。"
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
    state
        .store
        .set_subscription_enabled(&input.node_id, input.version, input.enabled, &target)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
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
    app_context::<AppState>(cx)
        .store
        .rename_subscription(&input.node_id, input.version, &input.name)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(see_other("/ui/subscriptions"))
}

#[derive(Deserialize)]
pub struct ModelQuery {
    node_id: String,
}

async fn import_provider(cx: &Cx, node_id: &str) -> Result<i64> {
    let node = app_context::<AppState>(cx)
        .store
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
    let state = app_context::<AppState>(cx);
    let id = import_provider(cx, &query.node_id).await?;
    let provider = state.store.get(id).await?;
    let target = state.store.probe_enabled_target(id).await?;
    let candidates = crate::app::model_catalog::query_models(target)
        .await
        .map_err(bad_request)?;
    let existing = state.store.list_models().await?;
    Ok(view! {
        <div class=(super::providers::PAGE_HEADING)>
            <div><h1>"导入订阅模型"</h1></div>
        </div>
        <p>
            (provider.name)
            "：选择需要导入的模型，导入后在 Model Routes 配置路由。"
        </p>
        <form
            class="rounded-lg border border-border bg-white p-5"
            method="post"
            action="/ui/subscriptions/import"
        >
            <input type="hidden" name="csrf" value=(state.csrf.clone())>
            <input type="hidden" name="node_id" value=(query.node_id.clone())>
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
            <button class=(super::providers::BUTTON) type="submit">
                "导入所选模型"
            </button>
            <a class=(super::providers::BUTTON) href="/ui/subscriptions">"返回"</a>
        </form>
    })
}

pub struct Import {
    csrf: String,
    node_id: String,
    models: Vec<String>,
}

#[route(POST "/ui/subscriptions/import")]
pub async fn save_import(
    cx: &Cx,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<topcoat::router::error::SeeOther> {
    let input = import_fields(fields);
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let id = import_provider(cx, &input.node_id).await?;
    let provider = state.store.get(id).await?;
    let candidates =
        crate::app::model_catalog::query_models(state.store.probe_enabled_target(id).await?)
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
    let existing = state.store.list_models().await?;
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
        state.store.create_models(mappings).await?;
    }
    Ok(see_other("/ui/models"))
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
