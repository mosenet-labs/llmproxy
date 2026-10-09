use super::*;
use llmproxy_store::health::{HealthCheckView, HealthStatus};
use topcoat::view::{emit, live};

fn status(checks: &[HealthCheckView]) -> (&'static str, TagTone) {
    let observed: Vec<_> = checks
        .iter()
        .filter(|check| check.config.enabled || check.result.is_some())
        .map(|check| check.status)
        .collect();
    if observed.is_empty() {
        return ("未探活", TagTone::Default);
    }
    if observed
        .iter()
        .all(|status| *status == HealthStatus::Unavailable)
    {
        return ("模型不可用", TagTone::Error);
    }
    if observed.iter().all(|status| status.blocks_calls()) {
        return ("上游不可用", TagTone::Error);
    }
    if observed.iter().any(|status| status.blocks_calls()) {
        return ("部分异常", TagTone::Warning);
    }
    if observed
        .iter()
        .all(|status| *status == HealthStatus::Healthy)
    {
        return ("探活正常", TagTone::Success);
    }
    if observed.iter().any(|status| {
        matches!(
            status,
            HealthStatus::Suspect | HealthStatus::RateLimited | HealthStatus::ProbeError
        )
    }) {
        return ("上游异常", TagTone::Warning);
    }
    ("待确认", TagTone::Default)
}

#[shard("/ui/_topcoat/runtime/shards/provider-health")]
pub async fn provider_health(cx: &Cx, provider_id: i64, enabled: bool) -> Result<impl View> {
    crate::app::request_connection(cx);
    let state = app_context::<AppState>(cx);
    Ok(live! {
        let mut changed = state.health.subscribe();
        loop {
            let mut checks = Vec::new();
            if enabled {
                checks = state.store.provider_health_checks(provider_id).await?;
            }
            let (label, tone) = status(&checks);
            let token = emit! {
                if enabled {
                    <span
                        title="汇总已配置探活或已有探测结果的模型；不改变 Provider 启用开关"
                        aria-label="上游健康状态"
                    >
                        tag(tone: tone, (label))
                    </span>
                }
            }?;
            if !enabled
                || topcoat::router::request::original_method(cx)
                    != topcoat::router::Method::POST {
                break Ok(token);
            }
            if changed.changed().await.is_err() {
                break Ok(token);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(status: HealthStatus) -> HealthCheckView {
        HealthCheckView {
            model_id: 1,
            protocol: Protocol::OpenAiChat,
            config: llmproxy_store::health::HealthCheckConfig {
                enabled: true,
                ..Default::default()
            },
            version: 1,
            status,
            last_probe_at: None,
            last_success_at: None,
            result: None,
            consecutive_failures: 0,
        }
    }
    #[test]
    fn upstream_health_distinguishes_model_absence_and_partial_failure() {
        assert_eq!(status(&[]).0, "未探活");
        assert_eq!(status(&[check(HealthStatus::Unhealthy)]).0, "上游不可用");
        assert_eq!(
            status(&[check(HealthStatus::Authentication)]).0,
            "上游不可用"
        );
        assert_eq!(status(&[check(HealthStatus::Unavailable)]).0, "模型不可用");
        assert_eq!(
            status(&[check(HealthStatus::Unhealthy), check(HealthStatus::Healthy)]).0,
            "部分异常"
        );
        assert_eq!(status(&[check(HealthStatus::Healthy)]).0, "探活正常");
        assert_eq!(status(&[check(HealthStatus::Stale)]).0, "待确认");
    }
}
