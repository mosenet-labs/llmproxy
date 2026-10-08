use std::{sync::Arc, time::Duration};

use llmproxy_core::protocol::Protocol;
use llmproxy_probe::{
    InferenceProbeTarget, ModelProber, ProbeResult, Reason, ThinkingMode, Verdict,
};
use llmproxy_store::{ProviderStore, health::HealthCheckJob};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::{self, MissedTickBehavior},
};

use crate::observability::ConsoleTelemetry;

pub(crate) struct ModelHealthService {
    store: ProviderStore,
    prober: ModelProber,
    telemetry: Arc<ConsoleTelemetry>,
    changed: watch::Sender<()>,
}

impl ModelHealthService {
    pub fn new(
        store: ProviderStore,
        telemetry: Arc<ConsoleTelemetry>,
    ) -> Result<Arc<Self>, reqwest::Error> {
        Ok(Arc::new(Self {
            store,
            prober: ModelProber::new()?,
            telemetry,
            changed: watch::channel(()).0,
        }))
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub async fn probe(
        &self,
        id: i64,
        protocol: Protocol,
        tokens: u32,
    ) -> Result<(String, ProbeResult), String> {
        let job = self
            .store
            .claim_health_checks(Some((id, protocol, tokens)), 1)
            .await
            .map_err(|error| error.to_string())?
            .pop()
            .ok_or_else(|| "模型正在探测或 Provider 已停用，请稍后重试".to_owned())?;
        let alias = job.route.alias.clone();
        let result = self.execute(&job).await?;
        Ok((alias, result))
    }

    async fn execute(&self, job: &HealthCheckJob) -> Result<ProbeResult, String> {
        let provider = job
            .route
            .provider
            .as_ref()
            .ok_or_else(|| "Provider 已停用".to_owned())?;
        let scheme = if provider.tls { "https" } else { "http" };
        let url = format!(
            "{scheme}://{}:{}{}",
            provider.host, provider.port, provider.upstream_path
        )
        .parse()
        .map_err(|_| "Provider 上游地址无效".to_owned())?;
        let timeout = Duration::from_millis(job.config.timeout_ms);
        let target = InferenceProbeTarget {
            url,
            protocol: job.route.protocol,
            secret: provider.secret.clone(),
            anthropic_version: provider.anthropic_version.clone(),
            messages_auth: provider.messages_auth,
            timeout,
        };
        let start = std::time::Instant::now();
        let result = time::timeout(
            timeout,
            self.prober.probe_model(
                &target,
                &job.route.upstream_model_id,
                job.config.max_output_tokens,
            ),
        )
        .await
        .unwrap_or(ProbeResult {
            verdict: Verdict::Inconclusive,
            reason: Some(Reason::Timeout),
            http_status: None,
            usage: None,
            elapsed: start.elapsed(),
            thinking_mode: ThinkingMode::DisabledRequested,
        });
        self.telemetry.model_probe(
            provider.id,
            &job.route.upstream_model_id,
            job.route.protocol,
            &result,
        );
        let mut random = [0u8; 8];
        let jitter = if getrandom::fill(&mut random).is_ok() {
            u64::from_ne_bytes(random) % (job.config.interval_seconds / 10 + 1)
        } else {
            0
        };
        if !self
            .store
            .finish_health_check(job, result, jitter)
            .await
            .map_err(|error| error.to_string())?
        {
            return Err("探测期间配置已改变，请重新探测".to_owned());
        }
        self.changed.send_replace(());
        Ok(result)
    }

    pub fn start(self: &Arc<Self>) -> Arc<HealthWorker> {
        let service = self.clone();
        Arc::new(HealthWorker(tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut jobs = JoinSet::new();
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let capacity = 4_usize.saturating_sub(jobs.len());
                        if capacity == 0 { continue; }
                        match service.store.claim_health_checks(None, capacity).await {
                            Ok(claimed) => for job in claimed {
                                let service = service.clone();
                                jobs.spawn(async move {
                                    if service.execute(&job).await.is_err() {
                                        tracing::warn!(component = "console", event_kind = "model_health", "scheduled model probe could not be persisted");
                                    }
                                });
                            },
                            Err(_) => tracing::warn!(component = "console", event_kind = "model_health", "cannot claim scheduled model probes"),
                        }
                    },
                    _ = jobs.join_next(), if !jobs.is_empty() => {},
                }
            }
        })))
    }
}

pub(crate) struct HealthWorker(JoinHandle<()>);

impl Drop for HealthWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) fn reason_label(reason: Option<Reason>) -> &'static str {
    match reason {
        Some(Reason::ModelNotFound) => "模型不存在",
        Some(Reason::Authentication) => "鉴权失败",
        Some(Reason::RateLimited) => "上游限流",
        Some(Reason::Timeout) => "请求超时",
        Some(Reason::Connection) => "连接失败",
        Some(Reason::InvalidRequest) => "探测参数不兼容",
        Some(Reason::UpstreamError) => "上游错误",
        Some(Reason::InvalidResponse) => "响应格式异常",
        Some(Reason::ThinkingStillEnabled) => "无法确认已关闭思考",
        None => "成功",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use llmproxy_store::{
        MessagesAuth, ModelMappingInput, ProviderInput, ProviderPaths,
        health::{HealthCheckConfig, HealthStatus},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn fixture(
        port: u16,
    ) -> (
        ProviderStore,
        std::path::PathBuf,
        i64,
        Arc<ModelHealthService>,
    ) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-health-service-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let store = ProviderStore::connect(
            &format!("sqlite:{}", directory.join("test.sqlite3").display()),
            &STANDARD.encode([8; 32]),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        let provider = store
            .create(ProviderInput {
                name: "Health".into(),
                paths: ProviderPaths {
                    openai_chat: Some("/infer".into()),
                    openai_responses: None,
                    anthropic_messages: None,
                    gemini: None,
                },
                host: "127.0.0.1".into(),
                port,
                tls: false,
                api_key: "test-secret".into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::OpenAiChat,
                anthropic_version: None,
                messages_auth: MessagesAuth::ApiKey,
                connect_timeout_ms: 1000,
                read_timeout_ms: 1000,
                write_timeout_ms: 1000,
            })
            .await
            .unwrap();
        let model = store
            .create_model(ModelMappingInput {
                thinking: Default::default(),
                alias: "health-model".into(),
                provider_id: provider.id,
                upstream_model_id: "upstream-test".into(),
                protocols: vec![Protocol::OpenAiChat],
                reference_price: None,
            })
            .await
            .unwrap();
        let service =
            ModelHealthService::new(store.clone(), Arc::new(ConsoleTelemetry::new())).unwrap();
        (store, directory, model.id, service)
    }

    #[tokio::test]
    async fn scheduler_and_manual_probe_share_request_and_persistence() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (store, directory, id, service) = fixture(listener.local_addr().unwrap().port()).await;
        let upstream = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0u8; 4096];
                    let size = stream.read(&mut buffer).await.unwrap();
                    assert!(size > 0);
                    request.extend_from_slice(&buffer[..size]);
                    if let Some(header_end) =
                        request.windows(4).position(|chunk| chunk == b"\r\n\r\n")
                    {
                        let headers =
                            String::from_utf8_lossy(&request[..header_end]).to_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if request.len() >= header_end + 4 + length {
                            let body: serde_json::Value =
                                serde_json::from_slice(&request[header_end + 4..]).unwrap();
                            assert_eq!(body["model"], "upstream-test");
                            assert_eq!(body["max_tokens"], 1);
                            assert!(headers.contains("authorization: bearer test-secret"));
                            break;
                        }
                    }
                }
                let body = r#"{"choices":[{"finish_reason":"length"}],"usage":{"prompt_tokens":8,"completion_tokens":1}}"#;
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        store
            .save_health_check(
                id,
                Protocol::OpenAiChat,
                0,
                HealthCheckConfig {
                    enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let worker = service.start();
        time::timeout(Duration::from_secs(5), async {
            loop {
                if store.model_health_checks(id).await.unwrap()[0].status == HealthStatus::Healthy {
                    break;
                }
                time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(store.claim_health_checks(None, 4).await.unwrap().is_empty());
        let mut changed = service.subscribe();
        let (alias, result) = service.probe(id, Protocol::OpenAiChat, 1).await.unwrap();
        time::timeout(Duration::from_secs(1), changed.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alias, "health-model");
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(
            store.model_health_checks(id).await.unwrap()[0]
                .result
                .unwrap()
                .usage
                .unwrap()
                .input,
            8
        );
        upstream.await.unwrap();
        drop(worker);
        tokio::task::yield_now().await;
        drop(service);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn total_timeout_is_persisted_and_worker_drop_cancels_inflight_probe() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (store, directory, id, service) = fixture(listener.local_addr().unwrap().port()).await;
        store
            .save_health_check(
                id,
                Protocol::OpenAiChat,
                0,
                HealthCheckConfig {
                    timeout_ms: 1000,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let manual = tokio::spawn({
            let service = service.clone();
            async move { service.probe(id, Protocol::OpenAiChat, 1).await.unwrap() }
        });
        let (mut stream, _) = time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let (_, result) = time::timeout(Duration::from_secs(3), manual)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.reason, Some(Reason::Timeout));
        assert_eq!(
            store.model_health_checks(id).await.unwrap()[0].status,
            HealthStatus::Suspect
        );
        let view = store.model_health_checks(id).await.unwrap().remove(0);
        store
            .save_health_check(
                id,
                Protocol::OpenAiChat,
                view.version,
                HealthCheckConfig {
                    enabled: true,
                    timeout_ms: 120000,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let worker = service.start();
        let (mut active, _) = time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .unwrap()
            .unwrap();
        drop(worker);
        time::timeout(Duration::from_secs(3), async {
            let mut buffer = [0u8; 4096];
            loop {
                if active.read(&mut buffer).await.unwrap() == 0 {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            store.model_health_checks(id).await.unwrap()[0].status,
            HealthStatus::Unknown
        );
        let _ = stream.shutdown().await;
        drop(service);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
