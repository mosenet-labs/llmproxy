use llmproxy_core::protocol::Protocol;
use llmproxy_probe::{ProbeResult, Reason, Verdict};

use crate::{ModelRoute, StoreError, StoreResult};

#[derive(Clone, Copy, Debug)]
pub struct HealthCheckConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub timeout_ms: u64,
    pub max_output_tokens: u32,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 300,
            timeout_ms: 30_000,
            max_output_tokens: 1,
        }
    }
}

impl HealthCheckConfig {
    pub fn validate(self) -> StoreResult<Self> {
        if !(30..=86400).contains(&self.interval_seconds)
            || !(1000..=120000).contains(&self.timeout_ms)
            || !(1..=1024).contains(&self.max_output_tokens)
        {
            return Err(StoreError::Validation(
                "探活间隔须为 30–86400 秒，超时 1000–120000 毫秒，输出上限 1–1024 token".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthStatus {
    Unknown,
    Healthy,
    Suspect,
    Unhealthy,
    Unavailable,
    Authentication,
    RateLimited,
    ProbeError,
    Stale,
}

impl HealthStatus {
    /// Fresh definitive absence, repeated transport failures, or invalid credentials block calls.
    pub fn blocks_calls(self) -> bool {
        matches!(
            self,
            Self::Unavailable | Self::Unhealthy | Self::Authentication
        )
    }
}

pub fn health_status(result: &ProbeResult, failures: u64) -> HealthStatus {
    match (result.verdict, result.reason) {
        (Verdict::Available, _) => HealthStatus::Healthy,
        (Verdict::Unavailable, _) => HealthStatus::Unavailable,
        (_, Some(Reason::Authentication)) => HealthStatus::Authentication,
        (_, Some(Reason::RateLimited)) => HealthStatus::RateLimited,
        (_, Some(Reason::Timeout | Reason::Connection | Reason::UpstreamError)) => {
            if failures >= 3 {
                HealthStatus::Unhealthy
            } else {
                HealthStatus::Suspect
            }
        }
        _ => HealthStatus::ProbeError,
    }
}

#[derive(Clone, Debug)]
pub struct HealthCheckView {
    pub model_id: i64,
    pub protocol: Protocol,
    pub config: HealthCheckConfig,
    pub version: u64,
    pub status: HealthStatus,
    pub last_probe_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub result: Option<ProbeResult>,
    pub consecutive_failures: u64,
}

// Server-only work item; route contains credentials and is never serialized.
pub struct HealthCheckJob {
    pub id: i64,
    pub generation: u64,
    pub model_version: u64,
    pub provider_version: u64,
    pub config: HealthCheckConfig,
    pub route: ModelRoute,
}
