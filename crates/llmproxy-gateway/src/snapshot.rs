use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    thread::{self, JoinHandle},
    time::Duration,
};

use llmproxy_core::{protocol::Protocol, provider::validate_upstream};
use llmproxy_store::{ActiveProvider, DatabaseConfig, ProviderStore};
use tokio::{runtime::Builder, sync::oneshot, time};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const DATABASE_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(30);

// Resolved credentials deliberately have no Debug or Serialize implementation.
#[derive(Clone)]
pub struct ResolvedProvider {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub secret: String,
    pub anthropic_version: Option<String>,
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
    providers: HashMap<Protocol, Arc<ResolvedProvider>>,
}

impl ProviderSnapshot {
    pub fn new(
        providers: impl IntoIterator<Item = (Protocol, ResolvedProvider)>,
    ) -> Result<Self, &'static str> {
        let mut snapshot = Self::default();
        for (protocol, provider) in providers {
            if snapshot
                .providers
                .insert(protocol, Arc::new(provider))
                .is_some()
            {
                return Err("multiple active providers for one protocol");
            }
        }
        Ok(snapshot)
    }

    fn from_database(providers: Vec<ActiveProvider>) -> Result<Self, &'static str> {
        let mut resolved = Vec::with_capacity(providers.len());
        for provider in providers {
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
            resolved.push((
                provider.protocol,
                ResolvedProvider {
                    host: provider.host,
                    port: provider.port,
                    tls: provider.tls,
                    secret: provider.secret,
                    anthropic_version: provider.anthropic_version,
                    connect_timeout_ms: provider.connect_timeout_ms,
                    read_timeout_ms: provider.read_timeout_ms,
                    write_timeout_ms: provider.write_timeout_ms,
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

    pub fn select(&self, protocol: Protocol) -> Option<Arc<ResolvedProvider>> {
        self.current
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .providers
            .get(&protocol)
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
            if config.backend().is_sqlite() {
                time::timeout(MIGRATION_TIMEOUT, store.migrate())
                    .await
                    .map_err(|_| "provider database migration timed out")?
                    .map_err(|_| "cannot migrate SQLite provider database")?;
            }
            let providers = time::timeout(DATABASE_TIMEOUT, store.load_active())
                .await
                .map_err(|_| "initial provider snapshot timed out")?
                .map_err(
                    |_| "cannot load provider snapshot; check database migrations and master key",
                )?;
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
                            time::timeout(DATABASE_TIMEOUT, store.load_active())
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
    use super::{ProviderSnapshot, ProviderSnapshots, ResolvedProvider};
    use llmproxy_core::protocol::Protocol;

    fn provider(host: &str, secret: &str) -> ResolvedProvider {
        ResolvedProvider {
            host: host.to_owned(),
            port: 443,
            tls: true,
            secret: secret.to_owned(),
            anthropic_version: None,
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
                provider("old.example", "old-dummy-key"),
            )])
            .unwrap(),
        );
        let in_flight = snapshots.select(Protocol::OpenAiChat).unwrap();
        snapshots.replace(
            ProviderSnapshot::new([(
                Protocol::OpenAiChat,
                provider("new.example", "new-dummy-key"),
            )])
            .unwrap(),
        );
        let next_request = snapshots.select(Protocol::OpenAiChat).unwrap();
        assert_eq!(in_flight.host, "old.example");
        assert_eq!(in_flight.secret, "old-dummy-key");
        assert_eq!(next_request.host, "new.example");
        assert_eq!(next_request.secret, "new-dummy-key");
        snapshots.replace(ProviderSnapshot::default());
        assert!(snapshots.select(Protocol::OpenAiChat).is_none());
        assert_eq!(next_request.host, "new.example");
    }

    #[test]
    fn duplicate_active_protocols_do_not_produce_a_snapshot() {
        assert!(
            ProviderSnapshot::new([
                (Protocol::OpenAiChat, provider("first.example", "dummy")),
                (Protocol::OpenAiChat, provider("second.example", "dummy")),
            ])
            .is_err()
        );
    }
}
