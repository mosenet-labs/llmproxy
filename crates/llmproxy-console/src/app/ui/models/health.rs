use super::*;
use llmproxy_store::health::{HealthCheckConfig, HealthCheckView, HealthStatus};

pub(super) fn status_label(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::Unknown => "待确认",
        HealthStatus::Healthy => "健康",
        HealthStatus::Suspect => "调用失败（待确认）",
        HealthStatus::Unhealthy => "连续调用失败",
        HealthStatus::Unavailable => "模型不可用",
        HealthStatus::Authentication => "鉴权异常",
        HealthStatus::RateLimited => "限流",
        HealthStatus::ProbeError => "探测异常",
        HealthStatus::Stale => "状态过期",
    }
}

pub(super) fn list_status(
    provider_enabled: bool,
    checks: &[HealthCheckView],
) -> (&'static str, TagTone) {
    if !provider_enabled {
        return ("Provider 已停用", TagTone::Warning);
    }
    for status in [
        HealthStatus::Unavailable,
        HealthStatus::Unhealthy,
        HealthStatus::Authentication,
        HealthStatus::ProbeError,
        HealthStatus::Suspect,
        HealthStatus::RateLimited,
        HealthStatus::Stale,
        HealthStatus::Unknown,
        HealthStatus::Healthy,
    ] {
        if checks
            .iter()
            .any(|check| check.config.enabled && check.status == status)
        {
            let tone = match status {
                HealthStatus::Healthy => TagTone::Success,
                HealthStatus::Unavailable
                | HealthStatus::Unhealthy
                | HealthStatus::Authentication => TagTone::Error,
                HealthStatus::Unknown => TagTone::Default,
                _ => TagTone::Warning,
            };
            return (status_label(status), tone);
        }
    }
    ("未探活", TagTone::Default)
}

fn time_label(value: Option<i64>) -> String {
    value
        .and_then(|time| chrono::DateTime::from_timestamp(time, 0))
        .map(|time| {
            time.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset"))
                .format("%m-%d %H:%M:%S +08:00")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_owned())
}

pub(super) struct HealthEditor {
    open: Signal<bool>,
    model_id: Signal<String>,
    alias: Signal<String>,
    revision: Signal<f64>,
    failure: Signal<String>,
}

impl HealthEditor {
    pub fn new(cx: &Cx) -> Self {
        Self {
            open: signal(cx, || false),
            model_id: signal(cx, String::new),
            alias: signal(cx, String::new),
            revision: signal(cx, || 0.0),
            failure: signal(cx, String::new),
        }
    }
}

pub(super) fn health_trigger(
    cx: &Cx,
    editor: &HealthEditor,
    model: &ModelMappingView,
) -> Attributes {
    let HealthEditor {
        open,
        model_id,
        alias,
        revision,
        failure,
    } = editor;
    let id = model.id.to_string();
    let name = model.alias.clone();
    attributes! {
        cx =>
        aria-haspopup="dialog"
        aria-controls="model-health-dialog"
        @click=$(|_event: Event| {
            model_id.set(id.to_owned());
            alias.set(name.to_owned());
            failure.set("".to_owned());
            revision.increment();
            open.set(true);
        })
    }
}

#[topcoat::view::component]
pub(super) async fn health_dialog(
    cx: &Cx,
    editor: &HealthEditor,
    refresh: &Signal<f64>,
    success: &Signal<String>,
) -> Result<impl View> {
    let HealthEditor {
        open,
        model_id,
        alias,
        revision,
        failure,
    } = editor;
    let busy = signal(cx, || false);
    let close = native_dialog_close_attributes(cx, "model-health-dialog");
    let dialog_revision = revision.clone();
    let dialog_open = open.clone();
    let workspace_refresh = refresh.clone();
    let dialog_success = success.clone();
    let dialog_failure = failure.clone();
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new("model-health-dialog", "探活配置"),
            open: Some(open),
            busy: &busy,
            language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="w-[min(640px,calc(100%_-_32px))]!" },
            <div class="max-h-[70dvh] overflow-y-auto px-6 py-5">
                <p class="mt-0 mb-2 break-words text-sm font-medium text-heading">
                    $(alias.get())
                </p>
                <p class="mt-0 mb-4 text-sm text-secondary">
                    "实际推理会消耗 token；按协议独立配置。"
                </p>
                <p class="text-sm text-danger" :hidden=$(failure.get().is_empty())>
                    $(failure.get())
                </p>
                model_health(
                    id: $(model_id.get()),
                    revision: $(revision.get()),
                    dialog_revision: $(dialog_revision),
                    open: $(dialog_open),
                    busy: $(busy),
                    refresh: $(workspace_refresh),
                    success: $(dialog_success),
                    failure: $(dialog_failure)
                )
            </div>
            <div class="flex justify-end border-t border-border px-6 py-4">
                <button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>
                    "关闭"
                </button>
            </div>
        )
    })
}

#[shard("/ui/_topcoat/runtime/shards/model-health")]
pub async fn model_health(
    cx: &Cx,
    id: String,
    revision: f64,
    dialog_revision: Signal<f64>,
    open: Signal<bool>,
    busy: Signal<bool>,
    refresh: Signal<f64>,
    success: Signal<String>,
    failure: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let state = app_context::<AppState>(cx);
    let checks = if id.is_empty() {
        Vec::new()
    } else {
        app_context::<AppState>(cx)
            .store
            .clone()
            .model_health_checks(id.parse::<i64>()?)
            .await?
    };
    Ok(view! {
        if !checks.is_empty() {
            <button
                class=(TEXT_LINK)
                type="button"
                :disabled=$(busy.get())
                @click=$(|_event: Event| dialog_revision.increment())
            >
                "刷新状态"
            </button>
        }
        #[key(check.protocol.as_str())]
        for check in &checks {
            health_form(
                check: check,
                csrf: state.csrf.as_str(),
                dialog_revision: &dialog_revision,
                open: &open,
                busy: &busy,
                refresh: &refresh,
                success: &success,
                failure: &failure
            )
        }
    })
}

#[topcoat::view::component]
async fn health_form(
    cx: &Cx,
    check: &HealthCheckView,
    csrf: &str,
    dialog_revision: &Signal<f64>,
    open: &Signal<bool>,
    busy: &Signal<bool>,
    refresh: &Signal<f64>,
    success: &Signal<String>,
    failure: &Signal<String>,
) -> Result<impl View> {
    let enabled = signal(cx, || check.config.enabled);
    let interval = signal(cx, || check.config.interval_seconds.to_string());
    let timeout = signal(cx, || check.config.timeout_ms.to_string());
    let tokens = signal(cx, || check.config.max_output_tokens.to_string());
    let id = check.model_id.to_string();
    let protocol = check.protocol.as_str().to_owned();
    let version = check.version.to_string();
    let csrf = csrf.to_owned();
    let checked = time_label(check.last_probe_at);
    let succeeded = time_label(check.last_success_at);
    let result = check.result.map(|result| {
        format!(
            "{} · {} ms · HTTP {} · 连续失败 {}",
            crate::model_health::reason_label(result.reason),
            result.elapsed.as_millis(),
            result
                .http_status
                .map_or_else(|| "—".to_owned(), |status| status.to_string()),
            check.consecutive_failures
        )
    });
    Ok(view! {
        <details class="mt-4 rounded-lg border border-border text-sm">
            <summary class="cursor-pointer px-4 py-3 text-heading">
                <span class="mr-3 font-medium">(protocol_label(check.protocol))</span>
                tag(
                    tone: if check.config.enabled {
                        TagTone::Success
                    } else {
                        TagTone::Default
                    },
                    (if check.config.enabled { "已配置" } else { "未配置" })
                )
                <span class="ml-3 text-secondary">(status_label(check.status))</span>
            </summary>
            <form
                class="flex flex-col gap-3 border-t border-border p-4"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    busy.set(true);
                    let outcome = save_health_check(
                        csrf,
                        id,
                        protocol,
                        version,
                        enabled.get(),
                        interval.get(),
                        timeout.get(),
                        tokens.get(),
                    ).await;
                    busy.set(false);
                    if outcome.is_ok() {
                        success.set(outcome.unwrap());
                        failure.set("".to_owned());
                        open.set(false);
                        refresh.increment();
                    } else {
                        failure.set(outcome.unwrap_err());
                    }
                })
            >
                <span class="text-secondary">
                    "最近检查："
                    (checked)
                </span>
                <span class="text-secondary">
                    "最近成功："
                    (succeeded)
                </span>
                if let Some(result) = result {
                    <span class="text-secondary">(result)</span>
                }
                <label>
                    <input
                        type="checkbox"
                        :checked=$(enabled.get())
                        @change=$(|event: Event| enabled.set(event.target.checked))
                    >
                    " 开启定时探活"
                </label>
                <label class="grid grid-cols-[140px_minmax(0,1fr)] items-center gap-3">
                    "间隔（秒）"
                    <input
                        class="h-9 min-w-0 rounded-md border border-control-border px-3 text-sm"
                        type="number"
                        min="30"
                        max="86400"
                        :value=$(interval.get())
                        @input=$(|event: Event| interval.set(event.target.value))
                    >
                </label>
                <label class="grid grid-cols-[140px_minmax(0,1fr)] items-center gap-3">
                    "超时（毫秒）"
                    <input
                        class="h-9 min-w-0 rounded-md border border-control-border px-3 text-sm"
                        type="number"
                        min="1000"
                        max="120000"
                        :value=$(timeout.get())
                        @input=$(|event: Event| timeout.set(event.target.value))
                    >
                </label>
                <label class="grid grid-cols-[140px_minmax(0,1fr)] items-center gap-3">
                    "输出上限（token）"
                    <input
                        class="h-9 min-w-0 rounded-md border border-control-border px-3 text-sm"
                        type="number"
                        min="1"
                        max="1024"
                        :value=$(tokens.get())
                        @input=$(|event: Event| tokens.set(event.target.value))
                    >
                </label>
                <div class="flex gap-3">
                    <button
                        class=(class!(BUTTON, PRIMARY))
                        type="submit"
                        :disabled=$(busy.get())
                    >
                        "保存配置"
                    </button>
                    <button
                        class=(BUTTON)
                        type="button"
                        :disabled=$(busy.get())
                        @click=$(async |_event: Event| {
                            busy.set(true);
                            let outcome = probe_saved_model(
                                csrf,
                                id,
                                protocol,
                                tokens.get(),
                            ).await;
                            busy.set(false);
                            if outcome.is_ok() {
                                success.set(outcome.unwrap());
                                failure.set("".to_owned());
                            } else {
                                failure.set(outcome.unwrap_err());
                            }
                            dialog_revision.increment();
                        })
                    >
                        "立即探测"
                    </button>
                </div>
            </form>
        </details>
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/save-health-check")]
pub async fn save_health_check(
    cx: &Cx,
    csrf: String,
    id: String,
    protocol: String,
    version: String,
    enabled: bool,
    interval: String,
    timeout: String,
    tokens: String,
) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let result = async {
        let invalid = || "探活配置格式无效".to_owned();
        let id = id.parse::<i64>().map_err(|_| invalid())?;
        let protocol = match protocol.as_str() {
            "openai_chat" => Protocol::OpenAiChat,
            "openai_responses" => Protocol::OpenAiResponses,
            "anthropic_messages" => Protocol::AnthropicMessages,
            "gemini" => Protocol::Gemini,
            _ => return Err(invalid()),
        };
        let config = HealthCheckConfig {
            enabled,
            interval_seconds: interval.parse().map_err(|_| invalid())?,
            timeout_ms: timeout.parse().map_err(|_| invalid())?,
            max_output_tokens: tokens.parse().map_err(|_| invalid())?,
        };
        app_context::<AppState>(cx)
            .store
            .clone()
            .save_health_check(
                id,
                protocol,
                version.parse().map_err(|_| invalid())?,
                config,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok("探活配置已保存".to_owned())
    }
    .await;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: HealthStatus, enabled: bool) -> HealthCheckView {
        HealthCheckView {
            model_id: 1,
            protocol: Protocol::OpenAiChat,
            config: HealthCheckConfig {
                enabled,
                ..Default::default()
            },
            version: 0,
            status,
            last_probe_at: None,
            last_success_at: None,
            result: None,
            consecutive_failures: 0,
        }
    }

    #[test]
    fn list_status_respects_provider_and_mixed_protocol_results() {
        assert_eq!(
            list_status(false, &[check(HealthStatus::Healthy, true)]).0,
            "Provider 已停用"
        );
        assert_eq!(
            list_status(true, &[check(HealthStatus::Healthy, false)]).0,
            "未探活"
        );
        assert_eq!(
            list_status(true, &[check(HealthStatus::Healthy, true)]).0,
            "健康"
        );
        for status in [
            HealthStatus::Unavailable,
            HealthStatus::Unhealthy,
            HealthStatus::Authentication,
            HealthStatus::ProbeError,
            HealthStatus::Suspect,
            HealthStatus::RateLimited,
            HealthStatus::Stale,
            HealthStatus::Unknown,
        ] {
            assert_eq!(
                list_status(
                    true,
                    &[check(HealthStatus::Healthy, true), check(status, true)]
                )
                .0,
                status_label(status)
            );
            assert_eq!(
                list_status(
                    true,
                    &[check(HealthStatus::Healthy, true), check(status, false)]
                )
                .0,
                "健康"
            );
        }
    }
}
