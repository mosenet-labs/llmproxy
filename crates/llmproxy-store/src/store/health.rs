use super::*;
use crate::{
    health::{HealthCheckConfig, HealthCheckJob, HealthCheckView, HealthStatus, health_status},
    model::ModelHealthCheck,
};
use llmproxy_probe::{ProbeResult, Reason, Verdict};

fn config(row: &ModelHealthCheck) -> HealthCheckConfig {
    HealthCheckConfig {
        enabled: row.enabled,
        interval_seconds: row.interval_seconds,
        timeout_ms: row.timeout_ms,
        max_output_tokens: row.max_output_tokens,
    }
}

async fn find_check(
    tx: &mut Transaction<'_>,
    model_id: i64,
    protocol: Protocol,
) -> StoreResult<Option<ModelHealthCheck>> {
    Ok(ModelHealthCheck::all()
        .filter(ModelHealthCheck::fields().model_id().eq(model_id))
        .filter(ModelHealthCheck::fields().protocol().eq(protocol.as_str()))
        .first()
        .exec(tx)
        .await?)
}

async fn create_check(
    tx: &mut Transaction<'_>,
    model_id: i64,
    protocol: Protocol,
) -> StoreResult<ModelHealthCheck> {
    let defaults = HealthCheckConfig::default();
    Ok(ModelHealthCheck::create()
        .model_id(model_id)
        .protocol(protocol.as_str())
        .enabled(false)
        .interval_seconds(defaults.interval_seconds)
        .timeout_ms(defaults.timeout_ms)
        .max_output_tokens(defaults.max_output_tokens)
        .next_probe_at(0_i64)
        .lease_until(0_i64)
        .generation(0_u64)
        .model_version(0_u64)
        .provider_version(0_u64)
        .consecutive_failures(0_u64)
        .exec(tx)
        .await?)
}

impl ProviderStore {
    pub async fn model_health_checks(&self, model_id: i64) -> StoreResult<Vec<HealthCheckView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let mapping = find_mapping(&mut tx, model_id).await?;
        let provider = find(&mut tx, mapping.provider_id).await?;
        let current_time = now()?;
        let mut views = Vec::new();
        for protocol in mapping.protocols() {
            let row = find_check(&mut tx, model_id, protocol).await?;
            let Some(row) = row else {
                views.push(HealthCheckView {
                    model_id,
                    protocol,
                    config: HealthCheckConfig::default(),
                    version: 0,
                    status: HealthStatus::Unknown,
                    last_probe_at: None,
                    last_success_at: None,
                    result: None,
                    consecutive_failures: 0,
                });
                continue;
            };
            let fresh_config =
                row.model_version == mapping.version && row.provider_version == provider.version;
            let result = if fresh_config {
                row.result_json.as_ref().map(|json| json.0)
            } else {
                None
            };
            let status = match result {
                None => HealthStatus::Unknown,
                Some(_)
                    if row.last_probe_at.is_some_and(|time| {
                        current_time - time
                            > (row.interval_seconds * 2 + row.timeout_ms.div_ceil(1000)) as i64
                    }) =>
                {
                    HealthStatus::Stale
                }
                Some(result) => health_status(&result, row.consecutive_failures),
            };
            views.push(HealthCheckView {
                model_id,
                protocol,
                config: config(&row),
                version: row.version,
                status,
                last_probe_at: row.last_probe_at,
                last_success_at: if fresh_config {
                    row.last_success_at
                } else {
                    None
                },
                result,
                consecutive_failures: if fresh_config {
                    row.consecutive_failures
                } else {
                    0
                },
            });
        }
        tx.commit().await?;
        Ok(views)
    }

    pub async fn save_health_check(
        &self,
        model_id: i64,
        protocol: Protocol,
        version: u64,
        input: HealthCheckConfig,
    ) -> StoreResult<()> {
        let input = input.validate()?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mapping = find_mapping(&mut tx, model_id).await?;
        if !mapping.protocols().contains(&protocol) {
            return Err(StoreError::Validation("模型未配置所选协议".into()));
        }
        let (mut row, created) = match find_check(&mut tx, model_id, protocol).await? {
            Some(row) => (row, false),
            None if version == 0 => (create_check(&mut tx, model_id, protocol).await?, true),
            None => return Err(StoreError::Conflict("探活配置已改变，请刷新".into())),
        };
        if !created && row.version != version {
            return Err(StoreError::Conflict("探活配置或状态已改变，请刷新".into()));
        }
        let generation = row.generation + 1;
        row.update()
            .enabled(input.enabled)
            .interval_seconds(input.interval_seconds)
            .timeout_ms(input.timeout_ms)
            .max_output_tokens(input.max_output_tokens)
            .next_probe_at(now()?)
            .generation(generation)
            .result_json(None::<toasty::Json<ProbeResult>>)
            .last_success_at(None::<i64>)
            .consecutive_failures(0_u64)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// All claimers use the existing cross-backend write lock; HTTP runs after commit.
    pub async fn claim_health_checks(
        &self,
        manual: Option<(i64, Protocol, u32)>,
        limit: usize,
    ) -> StoreResult<Vec<HealthCheckJob>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let current_time = now()?;
        let rows = if let Some((id, protocol, tokens)) = manual {
            if !(1..=1024).contains(&tokens) {
                return Err(StoreError::Validation("输出上限须为 1–1024 token".into()));
            }
            let mapping = find_mapping(&mut tx, id).await?;
            if !mapping.protocols().contains(&protocol) {
                return Err(StoreError::Validation("模型未配置所选协议".into()));
            }
            vec![match find_check(&mut tx, id, protocol).await? {
                Some(row) => row,
                None => create_check(&mut tx, id, protocol).await?,
            }]
        } else {
            ModelHealthCheck::all()
                .filter(ModelHealthCheck::fields().enabled().eq(true))
                .order_by(ModelHealthCheck::fields().next_probe_at().asc())
                .exec(&mut tx)
                .await?
        };
        let mut jobs = Vec::new();
        for mut row in rows {
            if jobs.len() >= limit {
                break;
            }
            if row.lease_until > current_time {
                continue;
            }
            let mapping = find_mapping(&mut tx, row.model_id).await?;
            let provider = find(&mut tx, mapping.provider_id).await?;
            let protocol = routes::protocol_from_str(&row.protocol)?;
            if !provider.enabled || !mapping.protocols().contains(&protocol) {
                continue;
            }
            let changed =
                row.model_version != mapping.version || row.provider_version != provider.version;
            if manual.is_none() && !changed && row.next_probe_at > current_time {
                continue;
            }
            let mut job_config = config(&row).validate()?;
            if let Some((_, _, tokens)) = manual {
                job_config.max_output_tokens = tokens;
            }
            let generation = row.generation + 1;
            row.update()
                .lease_until(current_time + job_config.timeout_ms.div_ceil(1000) as i64 + 10)
                .generation(generation)
                .exec(&mut tx)
                .await?;
            let thinking = mapping.thinking()?;
            let path = provider
                .paths()
                .get(protocol)
                .ok_or(StoreError::Internal)?
                .to_owned();
            jobs.push(HealthCheckJob {
                id: row.id,
                generation,
                model_version: mapping.version,
                provider_version: provider.version,
                config: job_config,
                route: ModelRoute {
                    model_id: Some(mapping.id),
                    alias: mapping.alias,
                    upstream_model_id: mapping.upstream_model_id,
                    enabled: true,
                    thinking,
                    provider: Some(self.active_provider(&provider, protocol, path)?),
                    protocol,
                },
            });
        }
        tx.commit().await?;
        Ok(jobs)
    }

    pub async fn finish_health_check(
        &self,
        job: &HealthCheckJob,
        result: ProbeResult,
        jitter_seconds: u64,
    ) -> StoreResult<bool> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let Some(mut row) = ModelHealthCheck::filter_by_id(job.id)
            .first()
            .exec(&mut tx)
            .await?
        else {
            return Ok(false);
        };
        if row.generation != job.generation || row.lease_until <= now()? {
            return Ok(false);
        }
        let mapping = find_mapping(&mut tx, row.model_id).await?;
        let provider = find(&mut tx, mapping.provider_id).await?;
        if mapping.version != job.model_version || provider.version != job.provider_version {
            row.update()
                .lease_until(0_i64)
                .next_probe_at(now()?)
                .exec(&mut tx)
                .await?;
            tx.commit().await?;
            return Ok(false);
        }
        let same_config =
            row.model_version == job.model_version && row.provider_version == job.provider_version;
        let failures = if matches!(
            result.reason,
            Some(Reason::Timeout | Reason::Connection | Reason::UpstreamError)
        ) {
            if same_config {
                row.consecutive_failures.saturating_add(1)
            } else {
                1
            }
        } else {
            0
        };
        let current_time = now()?;
        let success = if result.verdict == Verdict::Available {
            Some(current_time)
        } else if same_config {
            row.last_success_at
        } else {
            None
        };
        let next = current_time
            + row.interval_seconds as i64
            + jitter_seconds.min(row.interval_seconds / 10) as i64;
        row.update()
            .lease_until(0_i64)
            .next_probe_at(next)
            .model_version(job.model_version)
            .provider_version(job.provider_version)
            .last_probe_at(Some(current_time))
            .last_success_at(success)
            .result_json(Some(toasty::Json(result)))
            .consecutive_failures(failures)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use llmproxy_probe::ThinkingMode;

    fn result(verdict: Verdict, reason: Option<Reason>) -> ProbeResult {
        ProbeResult {
            verdict,
            reason,
            http_status: Some(200),
            usage: None,
            elapsed: Duration::from_millis(12),
            thinking_mode: ThinkingMode::DisabledRequested,
        }
    }

    #[test]
    fn config_boundaries_and_status_classification() {
        assert!(HealthCheckConfig::default().validate().is_ok());
        for input in [
            HealthCheckConfig {
                interval_seconds: 29,
                ..Default::default()
            },
            HealthCheckConfig {
                timeout_ms: 120001,
                ..Default::default()
            },
            HealthCheckConfig {
                max_output_tokens: 0,
                ..Default::default()
            },
        ] {
            assert!(input.validate().is_err());
        }
        for (reason, expected) in [
            (Reason::Authentication, HealthStatus::Authentication),
            (Reason::RateLimited, HealthStatus::RateLimited),
            (Reason::InvalidRequest, HealthStatus::ProbeError),
            (Reason::ThinkingStillEnabled, HealthStatus::ProbeError),
        ] {
            assert_eq!(
                health_status(&result(Verdict::Inconclusive, Some(reason)), 3),
                expected
            );
        }
        assert_eq!(
            health_status(
                &result(Verdict::Unavailable, Some(Reason::ModelNotFound)),
                0
            ),
            HealthStatus::Unavailable
        );
    }

    async fn fixture() -> (
        ProviderStore,
        std::path::PathBuf,
        ProviderInput,
        ModelMappingInput,
        i64,
    ) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-health-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
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
        let input = ProviderInput {
            name: "Health".into(),
            paths: crate::ProviderPaths {
                openai_chat: Some("/chat".into()),
                openai_responses: Some("/responses".into()),
                anthropic_messages: None,
                gemini: None,
            },
            host: "127.0.0.1".into(),
            port: 12345,
            tls: false,
            api_key: "test".into(),
            enabled: true,
            models_path: "/models".into(),
            models_protocol: Protocol::OpenAiChat,
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
        };
        let provider = store.create(input.clone()).await.unwrap();
        let mapping = ModelMappingInput {
            thinking: Default::default(),
            alias: "health-model".into(),
            provider_id: provider.id,
            upstream_model_id: "test".into(),
            protocols: vec![Protocol::OpenAiChat, Protocol::OpenAiResponses],
            reference_price: None,
        };
        let model = store.create_model(mapping.clone()).await.unwrap();
        (store, directory, input, mapping, model.id)
    }

    async fn manual(store: &ProviderStore, id: i64) -> HealthCheckJob {
        store
            .claim_health_checks(Some((id, Protocol::OpenAiChat, 1)), 1)
            .await
            .unwrap()
            .pop()
            .unwrap()
    }

    #[tokio::test]
    async fn health_results_threshold_recovery_protocol_isolation_and_staleness() {
        let (store, directory, _, _, id) = fixture().await;
        assert!(store.claim_health_checks(None, 4).await.unwrap().is_empty());
        for failures in 1..=3 {
            let job = manual(&store, id).await;
            assert!(
                store
                    .finish_health_check(
                        &job,
                        result(Verdict::Inconclusive, Some(Reason::Timeout)),
                        0
                    )
                    .await
                    .unwrap()
            );
            let checks = store.model_health_checks(id).await.unwrap();
            assert_eq!(checks[0].consecutive_failures, failures);
            assert_eq!(
                checks[0].status,
                if failures == 3 {
                    HealthStatus::Unhealthy
                } else {
                    HealthStatus::Suspect
                }
            );
            assert_eq!(checks[1].status, HealthStatus::Unknown);
        }
        let job = manual(&store, id).await;
        store
            .finish_health_check(&job, result(Verdict::Available, None), 0)
            .await
            .unwrap();
        let view = store.model_health_checks(id).await.unwrap().remove(0);
        assert_eq!(view.status, HealthStatus::Healthy);
        assert_eq!(view.consecutive_failures, 0);
        assert!(view.last_success_at.is_some());
        let mut connection = store.connection().await.unwrap();
        let mut row = ModelHealthCheck::filter_by_id(job.id)
            .first()
            .exec(&mut connection)
            .await
            .unwrap()
            .unwrap();
        row.update()
            .last_probe_at(Some(now().unwrap() - 1000))
            .exec(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            store.model_health_checks(id).await.unwrap()[0].status,
            HealthStatus::Stale
        );
        drop(connection);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn claims_are_exclusive_expire_and_reject_old_configuration() {
        let (store, directory, mut provider_input, mapping, id) = fixture().await;
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
        let other = ProviderStore::connect(
            &format!("sqlite:{}", directory.join("test.sqlite3").display()),
            &STANDARD.encode([8; 32]),
        )
        .await
        .unwrap();
        let first = tokio::spawn({
            let store = store.clone();
            async move { store.claim_health_checks(None, 4).await }
        });
        let second = tokio::spawn({
            let store = other.clone();
            async move { store.claim_health_checks(None, 4).await }
        });
        let (first, second) = tokio::join!(first, second);
        let first = first.unwrap();
        let second = second.unwrap();
        let mut jobs = first.unwrap();
        jobs.extend(second.unwrap());
        assert_eq!(jobs.len(), 1);
        let old = jobs.pop().unwrap();
        assert!(
            store
                .claim_health_checks(Some((id, Protocol::OpenAiChat, 1)), 1)
                .await
                .unwrap()
                .is_empty()
        );
        let mut connection = store.connection().await.unwrap();
        let mut row = ModelHealthCheck::filter_by_id(old.id)
            .first()
            .exec(&mut connection)
            .await
            .unwrap()
            .unwrap();
        row.update()
            .lease_until(0_i64)
            .exec(&mut connection)
            .await
            .unwrap();
        assert!(
            !store
                .finish_health_check(&old, result(Verdict::Available, None), 0)
                .await
                .unwrap()
        );
        let job = manual(&store, id).await;
        assert!(
            !store
                .finish_health_check(&old, result(Verdict::Available, None), 0)
                .await
                .unwrap()
        );
        store
            .finish_health_check(&job, result(Verdict::Available, None), 0)
            .await
            .unwrap();
        let provider = store.get(mapping.provider_id).await.unwrap();
        provider_input.host = "localhost".into();
        store
            .update(provider.id, provider.version, provider_input.clone())
            .await
            .unwrap();
        assert_eq!(
            store.model_health_checks(id).await.unwrap()[0].status,
            HealthStatus::Unknown
        );
        let changed_job = store
            .claim_health_checks(None, 4)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let model = store.get_model(id).await.unwrap();
        store
            .update_model(id, model.version, mapping.clone())
            .await
            .unwrap();
        assert!(
            !store
                .finish_health_check(&changed_job, result(Verdict::Available, None), 0)
                .await
                .unwrap()
        );
        let job = manual(&store, id).await;
        let view = store.model_health_checks(id).await.unwrap().remove(0);
        store
            .save_health_check(
                id,
                Protocol::OpenAiChat,
                view.version,
                HealthCheckConfig::default(),
            )
            .await
            .unwrap();
        assert!(
            !store
                .finish_health_check(&job, result(Verdict::Available, None), 0)
                .await
                .unwrap()
        );
        assert!(
            store
                .claim_health_checks(Some((id, Protocol::OpenAiChat, 1)), 1)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(store.claim_health_checks(None, 4).await.unwrap().is_empty());
        let model = store.get_model(id).await.unwrap();
        store.delete_model(id, model.version).await.unwrap();
        assert!(
            ModelHealthCheck::all()
                .exec(&mut connection)
                .await
                .unwrap()
                .is_empty()
        );
        drop(connection);
        drop(other);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn disabled_provider_and_removed_protocol_skip_scheduled_checks() {
        let (store, directory, mut provider_input, mut mapping, id) = fixture().await;
        store
            .save_health_check(
                id,
                Protocol::OpenAiResponses,
                0,
                HealthCheckConfig {
                    enabled: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        mapping.protocols = vec![Protocol::OpenAiChat];
        let model = store.get_model(id).await.unwrap();
        store
            .update_model(id, model.version, mapping.clone())
            .await
            .unwrap();
        assert!(store.claim_health_checks(None, 4).await.unwrap().is_empty());
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
        let provider = store.get(mapping.provider_id).await.unwrap();
        provider_input.enabled = false;
        store
            .update(provider.id, provider.version, provider_input)
            .await
            .unwrap();
        assert!(store.claim_health_checks(None, 4).await.unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
