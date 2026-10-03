use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    thread::{self, JoinHandle},
    time::Duration,
};

use llmproxy_core::{
    protocol::{MessagesAuth, Protocol},
    provider::validate_upstream,
};
use llmproxy_store::{DatabaseConfig, ModelRoute, ProviderStore, StoreError};
use tokio::{runtime::Builder, sync::oneshot, time};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const DATABASE_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(30);

// Resolved credentials deliberately have no Debug or Serialize implementation.
#[derive(Clone)]
pub struct ResolvedProvider {
    pub protocol: Protocol,
    pub upstream_path: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub secret: String,
    pub anthropic_version: Option<String>,
    pub messages_auth: MessagesAuth,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

impl ResolvedProvider {
    pub fn authority(&self) -> String {
        let default_port = if self.tls { 443 } else { 80 };
        if self.port == default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Default)]
pub struct ProviderSnapshot {
    models: HashMap<(Protocol, String), Arc<ResolvedModel>>,
}

pub struct ResolvedModel {
    pub upstream_model_id: String,
    pub provider: Option<Arc<ResolvedProvider>>,
}

impl ProviderSnapshot {
    pub fn new(
        models: impl IntoIterator<Item = (Protocol, String, ResolvedModel)>,
    ) -> Result<Self, &'static str> {
        let mut snapshot = Self::default();
        for (protocol, alias, model) in models {
            if snapshot
                .models
                .insert((protocol, alias), Arc::new(model))
                .is_some()
            {
                return Err("duplicate model alias for protocol");
            }
        }
        Ok(snapshot)
    }

    fn from_database(routes: Vec<ModelRoute>) -> Result<Self, &'static str> {
        let mut resolved = Vec::with_capacity(routes.len());
        for route in routes {
            let Some(provider) = route.provider else {
                resolved.push((
                    route.protocol,
                    route.alias,
                    ResolvedModel {
                        upstream_model_id: route.upstream_model_id,
                        provider: None,
                    },
                ));
                continue;
            };
            // Validate the entire candidate before replacing the live snapshot,
            // including rows that may have been edited outside the console.
            validate_upstream(
                &provider.host,
                provider.port,
                provider.anthropic_version.as_deref(),
                provider.connect_timeout_ms,
                provider.read_timeout_ms,
                provider.write_timeout_ms,
            )?;
            if provider.secret.trim().is_empty() || provider.secret.contains(['\r', '\n']) {
                return Err("invalid active provider credential");
            }
            if !provider.upstream_path.starts_with('/')
                || provider.upstream_path.starts_with("//")
                || provider.upstream_path.contains(['?', '#', '\\'])
            {
                return Err("invalid active provider path");
            }
            resolved.push((
                route.protocol,
                route.alias,
                ResolvedModel {
                    upstream_model_id: route.upstream_model_id,
                    provider: Some(Arc::new(ResolvedProvider {
                        protocol: provider.protocol,
                        upstream_path: provider.upstream_path,
                        host: provider.host,
                        port: provider.port,
                        tls: provider.tls,
                        secret: provider.secret,
                        anthropic_version: provider.anthropic_version,
                        messages_auth: provider.messages_auth,
                        connect_timeout_ms: provider.connect_timeout_ms,
                        read_timeout_ms: provider.read_timeout_ms,
                        write_timeout_ms: provider.write_timeout_ms,
                    })),
                },
            ));
        }
        Self::new(resolved)
    }
}

#[derive(Clone)]
pub struct ProviderSnapshots {
    current: Arc<RwLock<Arc<ProviderSnapshot>>>,
}

impl ProviderSnapshots {
    pub fn new(snapshot: ProviderSnapshot) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(snapshot))),
        }
    }

    pub fn select(&self, protocol: Protocol, alias: &str) -> Option<Arc<ResolvedModel>> {
        self.current
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .models
            .get(&(protocol, alias.to_owned()))
            .cloned()
    }

    fn replace(&self, snapshot: ProviderSnapshot) {
        let snapshot = Arc::new(snapshot);
        *self
            .current
            .write()
            .unwrap_or_else(|error| error.into_inner()) = snapshot;
    }

    pub fn database(config: &DatabaseConfig) -> Result<(Self, DatabaseRefresh), &'static str> {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "cannot initialize database runtime")?;
        let (store, initial) = runtime.block_on(async {
            let store = time::timeout(
                DATABASE_TIMEOUT,
                ProviderStore::connect(config.url(), config.master_key()),
            )
            .await
            .map_err(|_| "provider database connection timed out")?
            .map_err(|_| "cannot connect to provider database; check configuration")?;
            time::timeout(MIGRATION_TIMEOUT, store.migrate())
                .await
                .map_err(|_| "provider database migration timed out")?
                .map_err(|error| match error {
                    StoreError::Configuration(message) => message,
                    _ => "cannot migrate provider database; check schema and permissions",
                })?;
            tracing::info!(
                component = "gateway",
                event_kind = "runtime",
                "provider database schema ready"
            );
            let providers = time::timeout(DATABASE_TIMEOUT, store.load_model_routes())
                .await
                .map_err(|_| "initial provider snapshot timed out")?
                .map_err(|_| "cannot load provider snapshot; check provider records")?;
            let initial = ProviderSnapshot::from_database(providers)
                .map_err(|_| "initial provider snapshot is invalid")?;
            Ok::<_, &'static str>((store, initial))
        })?;
        let snapshots = Self::new(initial);
        let background = snapshots.clone();
        let (stop, mut stopping) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("provider-snapshot-refresh".to_owned())
            .spawn(move || {
                runtime.block_on(async move {
                    let mut interval = time::interval_at(
                        time::Instant::now() + REFRESH_INTERVAL,
                        REFRESH_INTERVAL,
                    );
                    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
                    let mut unavailable = false;
                    loop {
                        let next_snapshot = async {
                            interval.tick().await;
                            time::timeout(DATABASE_TIMEOUT, store.load_model_routes())
                                .await
                                .map_err(|_| ())?
                                .map_err(|_| ())
                                .and_then(|providers| {
                                    ProviderSnapshot::from_database(providers).map_err(|_| ())
                                })
                        };
                        let loaded = tokio::select! {
                            _ = &mut stopping => break,
                            loaded = next_snapshot => loaded,
                        };
                        match loaded {
                            Ok(snapshot) => {
                                background.replace(snapshot);
                                if unavailable {
                                    crate::observability::snapshot_refresh(true);
                                    unavailable = false;
                                }
                            }
                            Err(()) => {
                                // Database errors may contain URLs, SQL values, or keys.
                                if !unavailable {
                                    crate::observability::snapshot_refresh(false);
                                    unavailable = true;
                                }
                            }
                        }
                    }
                });
            })
            .map_err(|_| "cannot start provider snapshot refresh")?;
        Ok((
            snapshots,
            DatabaseRefresh {
                stop: Some(stop),
                thread: Some(thread),
            },
        ))
    }
}

pub struct DatabaseRefresh {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DatabaseRefresh {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{ProviderSnapshot, ProviderSnapshots, ResolvedModel, ResolvedProvider};
    use llmproxy_core::protocol::{MessagesAuth, Protocol};

    fn provider(host: &str, secret: &str) -> ResolvedProvider {
        ResolvedProvider {
            protocol: Protocol::OpenAiChat,
            upstream_path: "/v1/chat/completions".to_owned(),
            host: host.to_owned(),
            port: 443,
            tls: true,
            secret: secret.to_owned(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            connect_timeout_ms: 1000,
            read_timeout_ms: 2000,
            write_timeout_ms: 3000,
        }
    }

    #[test]
    fn replacement_keeps_the_selected_request_provider_unchanged() {
        let snapshots = ProviderSnapshots::new(
            ProviderSnapshot::new([(
                Protocol::OpenAiChat,
                "public/one".to_owned(),
                ResolvedModel {
                    upstream_model_id: "one".to_owned(),
                    provider: Some(Arc::new(provider("old.example", "old-dummy-key"))),
                },
            )])
            .unwrap(),
        );
        let in_flight = snapshots
            .select(Protocol::OpenAiChat, "public/one")
            .unwrap();
        snapshots.replace(
            ProviderSnapshot::new([(
                Protocol::OpenAiChat,
                "public/one".to_owned(),
                ResolvedModel {
                    upstream_model_id: "two".to_owned(),
                    provider: Some(Arc::new(provider("new.example", "new-dummy-key"))),
                },
            )])
            .unwrap(),
        );
        let next_request = snapshots
            .select(Protocol::OpenAiChat, "public/one")
            .unwrap();
        assert_eq!(in_flight.provider.as_ref().unwrap().host, "old.example");
        assert_eq!(in_flight.provider.as_ref().unwrap().secret, "old-dummy-key");
        assert_eq!(next_request.provider.as_ref().unwrap().host, "new.example");
        assert_eq!(
            next_request.provider.as_ref().unwrap().secret,
            "new-dummy-key"
        );
        snapshots.replace(ProviderSnapshot::default());
        assert!(
            snapshots
                .select(Protocol::OpenAiChat, "public/one")
                .is_none()
        );
        assert_eq!(next_request.provider.as_ref().unwrap().host, "new.example");
    }

    #[test]
    fn duplicate_active_protocols_do_not_produce_a_snapshot() {
        assert!(
            ProviderSnapshot::new([
                (
                    Protocol::OpenAiChat,
                    "same".to_owned(),
                    ResolvedModel {
                        upstream_model_id: "one".to_owned(),
                        provider: Some(Arc::new(provider("first.example", "dummy")))
                    }
                ),
                (
                    Protocol::OpenAiChat,
                    "same".to_owned(),
                    ResolvedModel {
                        upstream_model_id: "two".to_owned(),
                        provider: Some(Arc::new(provider("second.example", "dummy")))
                    }
                ),
            ])
            .is_err()
        );
    }
}
