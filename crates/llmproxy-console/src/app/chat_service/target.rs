//! 页面选择和后台生成共用模型／路由校验，避免两条路径的能力判断不一致。

use llmproxy_core::{protocol::Protocol, thinking::Support};
use llmproxy_store::{ModelRouteView, ProviderStore};

/// 路由至少包含一个启用的目标才可用于对话。
pub(crate) fn route_available(route: &ModelRouteView) -> bool {
    route.enabled
        && route
            .targets
            .iter()
            .any(|target| target.enabled && target.model.provider_enabled)
}

/// 从保存的选择解析别名、入口协议及模型能力，不修改当前会话选择。
pub(crate) async fn resolve(
    store: &ProviderStore,
    selection: &str,
) -> Result<(String, Vec<Protocol>, bool, Support), String> {
    if let Some(id) = selection.strip_prefix("route:") {
        let id = id.parse::<i64>().map_err(|_| "请选择有效的模型路由")?;
        let route = store
            .list_routes()
            .await
            .map_err(|_| "无法读取模型路由配置")?
            .into_iter()
            .find(|route| route.id == id)
            .ok_or("模型路由已不存在")?;
        let available = route_available(&route);
        let support = route_support(&route);
        Ok((route.name, vec![route.protocol], available, support))
    } else {
        let id = selection.parse::<i64>().map_err(|_| "请选择有效的模型")?;
        let model = store.get_model(id).await.map_err(|_| "无法读取模型配置")?;
        Ok((
            model.alias,
            model.protocols,
            model.provider_enabled,
            model.thinking.support,
        ))
    }
}

/// 路由能力采用可用目标的交集；实际请求仍由 Gateway 校验选中的模型快照。
fn route_support(route: &ModelRouteView) -> Support {
    let supports: Vec<_> = route
        .targets
        .iter()
        .filter(|t| {
            t.enabled
                && t.model.provider_enabled
                && t.model.protocols.contains(&route.provider_protocol)
        })
        .map(|t| t.model.thinking.support)
        .collect();
    if supports.is_empty() || supports.contains(&Support::Unknown) {
        Support::Unknown
    } else if supports.contains(&Support::Unsupported) {
        Support::Unsupported
    } else if supports.contains(&Support::AlwaysOn) {
        Support::AlwaysOn
    } else {
        Support::Switchable
    }
}
