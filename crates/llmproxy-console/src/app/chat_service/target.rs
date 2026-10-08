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

/// Chat 的提示策略独立于 Gateway 路由；过期、未知与瞬时错误不禁止发送。
#[derive(Clone, Debug)]
pub(crate) struct Health {
    pub blocked: bool,
    pub warning: bool,
    pub severe: bool,
    pub label: String,
}

impl Health {
    fn unknown() -> Self {
        Self {
            blocked: false,
            warning: false,
            severe: false,
            label: "未探活".into(),
        }
    }
}

fn from_status(status: llmproxy_store::health::HealthStatus) -> Health {
    use llmproxy_store::health::HealthStatus::*;
    let (label, warning, severe) = match status {
        Healthy => ("健康", false, false),
        Unknown => ("待确认", false, false),
        Unavailable => ("模型不存在", true, true),
        Unhealthy => ("连续探活失败", true, true),
        Authentication => ("鉴权异常", true, true),
        Suspect => ("最近探活失败", true, false),
        RateLimited => ("上游限流", true, false),
        ProbeError => ("探测异常", true, false),
        Stale => ("探活结果已过期", true, false),
    };
    Health {
        blocked: status.blocks_calls(),
        warning,
        severe,
        label: label.into(),
    }
}

async fn model_health(
    store: &ProviderStore,
    id: i64,
    protocol: Protocol,
    enabled: bool,
) -> Result<Health, String> {
    if !enabled {
        return Ok(Health {
            blocked: true,
            warning: true,
            severe: true,
            label: "Provider 已停用".into(),
        });
    }
    Ok(store
        .model_health_checks(id)
        .await
        .map_err(|_| "无法读取探活状态")?
        .into_iter()
        .find(|check| {
            check.protocol == protocol && (check.config.enabled || check.result.is_some())
        })
        .map_or_else(Health::unknown, |check| from_status(check.status)))
}

pub(crate) async fn health(
    store: &ProviderStore,
    selection: &str,
    protocol: Protocol,
) -> Result<Health, String> {
    if let Some(id) = selection.strip_prefix("route:") {
        let id = id.parse::<i64>().map_err(|_| "模型路由 ID 无效")?;
        let route = store
            .list_routes()
            .await
            .map_err(|_| "无法读取模型路由")?
            .into_iter()
            .find(|route| route.id == id)
            .ok_or("模型路由已不存在")?;
        if !route.enabled || route.protocol != protocol {
            return Ok(Health {
                blocked: true,
                warning: true,
                severe: true,
                label: "路由或协议已停用".into(),
            });
        }
        let mut candidates = Vec::new();
        for target in route.targets.iter().filter(|target| {
            target.enabled
                && target.model.provider_enabled
                && target.model.protocols.contains(&route.provider_protocol)
        }) {
            candidates
                .push(model_health(store, target.model.id, route.provider_protocol, true).await?);
        }
        let Some(first) = candidates.first().cloned() else {
            return Ok(Health {
                blocked: true,
                warning: true,
                severe: true,
                label: "路由无启用目标".into(),
            });
        };
        if candidates.iter().all(|health| health.blocked) {
            return Ok(Health {
                label: "路由所有候选均不可用".into(),
                ..first
            });
        }
        if first.blocked {
            return Ok(Health {
                blocked: true,
                warning: true,
                severe: true,
                label: "首选目标不可用；路由尚未按探活自动切换".into(),
            });
        }
        Ok(first)
    } else {
        let id = selection.parse::<i64>().map_err(|_| "模型 ID 无效")?;
        let model = store.get_model(id).await.map_err(|_| "模型配置已不存在")?;
        if !model.protocols.contains(&protocol) {
            return Ok(Health {
                blocked: true,
                warning: true,
                severe: true,
                label: "模型未配置此协议".into(),
            });
        }
        model_health(store, id, protocol, model.provider_enabled).await
    }
}

pub(crate) async fn choose_protocol(
    store: &ProviderStore,
    selection: &str,
    current: &str,
) -> Result<String, String> {
    let (_, mut protocols, available, _) = resolve(store, selection).await?;
    if !available {
        return Ok(String::new());
    }
    if let Some(index) = protocols
        .iter()
        .position(|protocol| protocol.as_str() == current)
    {
        let preferred = protocols.remove(index);
        protocols.insert(0, preferred);
    }
    for protocol in protocols {
        if !health(store, selection, protocol).await?.blocked {
            return Ok(protocol.as_str().to_owned());
        }
    }
    Ok(String::new())
}

pub(crate) async fn reprobe(
    store: &ProviderStore,
    service: &crate::model_health::ModelHealthService,
    selection: &str,
    protocol: Protocol,
) -> Result<(), String> {
    let (id, upstream_protocol) = if let Some(id) = selection.strip_prefix("route:") {
        let id = id.parse::<i64>().map_err(|_| "路由 ID 无效")?;
        let route = store
            .list_routes()
            .await
            .map_err(|_| "无法读取路由")?
            .into_iter()
            .find(|route| route.id == id)
            .ok_or("模型路由已不存在")?;
        let target = route
            .targets
            .iter()
            .find(|target| {
                target.enabled
                    && target.model.provider_enabled
                    && target.model.protocols.contains(&route.provider_protocol)
            })
            .ok_or("路由无启用目标")?;
        (target.model.id, route.provider_protocol)
    } else {
        (
            selection.parse::<i64>().map_err(|_| "模型 ID 无效")?,
            protocol,
        )
    };
    let tokens = store
        .model_health_checks(id)
        .await
        .map_err(|_| "无法读取探活配置")?
        .into_iter()
        .find(|check| check.protocol == upstream_protocol)
        .ok_or("模型未配置此协议")?
        .config
        .max_output_tokens;
    service.probe(id, upstream_protocol, tokens).await?;
    Ok(())
}

#[cfg(test)]
mod health_tests {
    use super::*;
    use llmproxy_store::health::HealthStatus;

    #[test]
    fn confirmed_unavailability_blocks_health_selection() {
        for status in [
            HealthStatus::Unavailable,
            HealthStatus::Unhealthy,
            HealthStatus::Authentication,
        ] {
            assert!(from_status(status).blocked);
        }
        for status in [
            HealthStatus::Healthy,
            HealthStatus::Unknown,
            HealthStatus::Suspect,
            HealthStatus::RateLimited,
            HealthStatus::ProbeError,
            HealthStatus::Stale,
        ] {
            assert!(!from_status(status).blocked);
        }
        assert!(from_status(HealthStatus::Authentication).severe);
        assert!(from_status(HealthStatus::Unhealthy).warning);
        assert!(!from_status(HealthStatus::Unknown).warning);
    }
}
