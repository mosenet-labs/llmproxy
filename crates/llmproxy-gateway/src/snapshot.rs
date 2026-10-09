use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
use tokio_util::sync::CancellationToken;

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const DATABASE_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(30);

const PROTOCOLS: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "chat",
        Protocol::OpenAiResponses => "responses",
        Protocol::AnthropicMessages => "messages",
        Protocol::Gemini => "gemini",
    }
}

fn protocol_path(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Gemini => "/v1beta/models/{model}:generateContent",
        _ => protocol.upstream_path(),
    }
}

#[derive(serde::Serialize)]
pub struct CatalogModel {
    id: String,
    name: String,
    object: &'static str,
    created: u64,
    owned_by: &'static str,
    protocols: BTreeMap<&'static str, &'static str>,
}

// Resolved credentials deliberately have no Debug or Serialize implementation.
#[derive(Clone)]
pub struct ResolvedProvider {
    pub id: i64,
    pub name: String,
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
    groups: HashMap<i64, ProviderSnapshot>,
    route_ids: HashMap<(Protocol, String), i64>,
    model_ids: HashMap<(Protocol, String), i64>,
}

pub struct ResolvedModel {
    pub blocked: bool,
    pub thinking: llmproxy_core::thinking::Config,
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

    fn select(&self, protocol: Protocol, alias: &str) -> Option<Arc<ResolvedModel>> {
        self.select_allowed(protocol, alias, |_| true)
    }

    fn select_allowed(
        &self,
        protocol: Protocol,
        alias: &str,
        allowed: impl Fn(Protocol) -> bool,
    ) -> Option<Arc<ResolvedModel>> {
        let get = |kind| {
            self.models
                .get(&(kind, alias.to_owned()))
                .filter(|_| allowed(kind))
        };
        let exact = get(protocol);
        // Explicitly disabled configuration must not be bypassed by bridging.
        if let Some(model) = exact
            && (model.provider.is_none() || !model.blocked)
        {
            return Some(model.clone());
        }
        PROTOCOLS
            .iter()
            .filter_map(|kind| get(*kind))
            .find(|model| model.provider.is_some() && !model.blocked)
            .or(exact)
            .or_else(|| PROTOCOLS.iter().find_map(|kind| get(*kind)))
            .cloned()
    }

    fn catalog(&self) -> Vec<CatalogModel> {
        let mut entries = BTreeMap::new();
        for (_, alias) in self.models.keys() {
            entries.entry(alias.clone()).or_insert_with(|| {
                let protocols = PROTOCOLS
                    .iter()
                    .filter_map(|protocol| {
                        self.select(*protocol, alias)
                            .filter(|model| model.provider.is_some() && !model.blocked)
                            .map(|_| (protocol_name(*protocol), protocol_path(*protocol)))
                    })
                    .collect::<BTreeMap<_, _>>();
                CatalogModel {
                    id: alias.clone(),
                    name: alias.clone(),
                    object: "model",
                    created: 0,
                    owned_by: "llmproxy",
                    protocols,
                }
            });
        }
        entries
            .into_values()
            .filter(|model| !model.protocols.is_empty())
            .collect()
    }

    fn allows(&self, identity: &llmproxy_store::CallIdentity, key: &(Protocol, String)) -> bool {
        identity.all_routes
            || if let Some(id) = self.route_ids.get(key) {
                identity.route_ids.contains(id)
            } else {
                self.model_ids
                    .get(key)
                    .is_some_and(|id| identity.model_ids.contains(id))
            }
    }

    async fn load(store: &ProviderStore) -> Result<Self, ()> {
        let mut root = Self::default();
        for group in store.list_groups().await.map_err(|_| ())? {
            if !group.enabled {
                continue;
            }
            let scoped = store.for_group(group.id);
            let routes = scoped.load_model_routes().await.map_err(|_| ())?;
            let ids: HashSet<_> = routes.iter().filter_map(|route| route.model_id).collect();
            let mut unavailable = HashSet::new();
            for id in ids {
                for check in scoped.model_health_checks(id).await.map_err(|_| ())? {
                    if check.status.blocks_calls() {
                        unavailable.insert((id, check.protocol));
                    }
                }
            }
            let mut snapshot = Self::from_database(routes, &unavailable).map_err(|_| ())?;
            snapshot.route_ids = scoped
                .list_routes()
                .await
                .map_err(|_| ())?
                .into_iter()
                .map(|route| ((route.protocol, route.name), route.id))
                .collect();
            snapshot.model_ids = scoped
                .list_models()
                .await
                .map_err(|_| ())?
                .into_iter()
                .flat_map(|model| {
                    model
                        .protocols
                        .into_iter()
                        .map(move |protocol| ((protocol, model.alias.clone()), model.id))
                })
                .collect();
            root.groups.insert(group.id, snapshot);
        }
        Ok(root)
    }

    fn from_database(
        routes: Vec<ModelRoute>,
        unavailable: &HashSet<(i64, Protocol)>,
    ) -> Result<Self, &'static str> {
        let mut resolved = Vec::with_capacity(routes.len());
        for route in routes {
            let Some(provider) = route.provider else {
                resolved.push((
                    route.protocol,
                    route.alias,
                    ResolvedModel {
                        blocked: false,
                        thinking: route.thinking,
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
                    blocked: route
                        .model_id
                        .is_some_and(|id| unavailable.contains(&(id, provider.protocol))),
                    thinking: route.thinking,
                    upstream_model_id: route.upstream_model_id,
                    provider: Some(Arc::new(ResolvedProvider {
                        id: provider.id,
                        name: provider.name.clone(),
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

    #[cfg(test)]
    pub fn select(&self, protocol: Protocol, alias: &str) -> Option<Arc<ResolvedModel>> {
        self.current
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .select(protocol, alias)
    }

    pub fn select_for(
        &self,
        group_id: i64,
        identity: Option<&llmproxy_store::CallIdentity>,
        protocol: Protocol,
        alias: &str,
    ) -> Result<(Arc<ResolvedModel>, Option<i64>), u16> {
        let current = self
            .current
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let snapshot = current
            .groups
            .get(&group_id)
            .or_else(|| (group_id == 1 && current.groups.is_empty()).then_some(current.as_ref()))
            .ok_or(404u16)?;
        if identity.is_none() {
            return snapshot
                .select(protocol, alias)
                .map(|model| (model, None))
                .ok_or(404);
        }
        let identity = identity.unwrap();
        let allowed = |key: &(Protocol, String)| snapshot.allows(identity, key);
        if !snapshot.models.keys().any(|key| key.1 == alias) {
            return Err(404);
        }
        let model = snapshot
            .select_allowed(protocol, alias, |kind| allowed(&(kind, alias.to_owned())))
            .ok_or(403u16)?;
        let route_id = snapshot
            .models
            .iter()
            .find(|(_, candidate)| Arc::ptr_eq(candidate, &model))
            .and_then(|(key, _)| snapshot.route_ids.get(key))
            .copied();
        Ok((model, route_id))
    }

    pub fn catalog_for(&self, identity: &llmproxy_store::CallIdentity) -> Vec<CatalogModel> {
        let current = self
            .current
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let Some(snapshot) = current.groups.get(&identity.group_id) else {
            return Vec::new();
        };
        ProviderSnapshot {
            models: snapshot
                .models
                .iter()
                .filter(|(key, _)| snapshot.allows(identity, key))
                .map(|(key, model)| (key.clone(), model.clone()))
                .collect(),
            ..Default::default()
        }
        .catalog()
    }

    fn replace(&self, snapshot: ProviderSnapshot) {
        let snapshot = Arc::new(snapshot);
        *self
            .current
            .write()
            .unwrap_or_else(|error| error.into_inner()) = snapshot;
    }

    pub fn database(
        config: &DatabaseConfig,
    ) -> Result<(Self, DatabaseRefresh, ProviderStore), &'static str> {
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
            let initial = time::timeout(DATABASE_TIMEOUT, ProviderSnapshot::load(&store))
                .await
                .map_err(|_| "initial provider snapshot timed out")?
                .map_err(|_| "cannot load provider snapshot; check provider records")?;
            Ok::<_, &'static str>((store, initial))
        })?;
        let snapshots = Self::new(initial);
        let background = snapshots.clone();
        let gateway_store = store.clone();
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let stopped = CancellationToken::new();
        let finished = stopped.clone();
        let (shutdown, shutting_down) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("provider-snapshot-refresh".to_owned())
            .spawn(move || {
                let _finished = finished.clone().drop_guard();
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
                            time::timeout(DATABASE_TIMEOUT, ProviderSnapshot::load(&store))
                                .await
                                .map_err(|_| ())?
                                .map_err(|_| ())
                        };
                        let loaded = tokio::select! {
                            biased;
                            _ = stopping.cancelled() => break,
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
                    tracing::info!(
                        component = "gateway",
                        event_kind = "runtime",
                        "provider snapshot refresh stopped"
                    );
                    finished.cancel();
                    // Toasty's pooled connection workers run on this runtime too.
                    // Keep them alive until HTTP and console runtimes have stopped.
                    let _ = shutting_down.await;
                });
            })
            .map_err(|_| "cannot start provider snapshot refresh")?;
        Ok((
            snapshots,
            DatabaseRefresh {
                stop,
                stopped,
                shutdown: Some(shutdown),
                thread: Some(thread),
            },
            gateway_store,
        ))
    }
}

pub struct DatabaseRefresh {
    stop: CancellationToken,
    stopped: CancellationToken,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(unix)]
impl DatabaseRefresh {
    pub fn shutdown_signal(&self) -> impl pingora::server::ShutdownSignalWatch + use<> {
        DatabaseShutdown {
            stop: self.stop.clone(),
            stopped: self.stopped.clone(),
        }
    }
}

#[cfg(unix)]
struct DatabaseShutdown {
    stop: CancellationToken,
    stopped: CancellationToken,
}

#[cfg(unix)]
#[async_trait::async_trait]
impl pingora::server::ShutdownSignalWatch for DatabaseShutdown {
    async fn recv(&self) -> pingora::server::ShutdownSignal {
        use pingora::server::UnixShutdownSignalWatch;

        let signal = UnixShutdownSignalWatch.recv().await;
        self.stop.cancel();
        self.stopped.cancelled().await;
        signal
    }
}

impl Drop for DatabaseRefresh {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
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

    #[test]
    fn stopping_refresh_keeps_database_workers_alive_until_services_exit() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-refresh-shutdown-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = llmproxy_store::DatabaseConfig::from_values(
            &format!("sqlite:{}", directory.join("providers.sqlite3").display()),
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        )
        .unwrap();
        let (_, refresh, store) = ProviderSnapshots::database(&config).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            refresh.stop.cancel();
            tokio::time::timeout(super::DATABASE_TIMEOUT, refresh.stopped.cancelled())
                .await
                .expect("snapshot refresh must stop before service runtimes shut down");
            assert!(!refresh.thread.as_ref().unwrap().is_finished());
            assert_eq!(store.list_groups().await.unwrap().len(), 1);
        });
        drop(store);
        drop(runtime);
        drop(refresh);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn provider(host: &str, secret: &str) -> ResolvedProvider {
        ResolvedProvider {
            id: 0,
            name: "fixture".into(),
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
                    blocked: false,
                    thinking: Default::default(),
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
                    blocked: false,
                    thinking: Default::default(),
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
                        blocked: false,
                        thinking: Default::default(),
                        upstream_model_id: "one".to_owned(),
                        provider: Some(Arc::new(provider("first.example", "dummy")))
                    }
                ),
                (
                    Protocol::OpenAiChat,
                    "same".to_owned(),
                    ResolvedModel {
                        blocked: false,
                        thinking: Default::default(),
                        upstream_model_id: "two".to_owned(),
                        provider: Some(Arc::new(provider("second.example", "dummy")))
                    }
                ),
            ])
            .is_err()
        );
    }
    fn model(protocol: Protocol, id: &str, blocked: bool, enabled: bool) -> ResolvedModel {
        let mut upstream = provider("fixture.example", "dummy-key");
        upstream.protocol = protocol;
        upstream.upstream_path = protocol.upstream_path().into();
        ResolvedModel {
            blocked,
            thinking: Default::default(),
            upstream_model_id: id.into(),
            provider: enabled.then(|| Arc::new(upstream)),
        }
    }

    #[test]
    fn cross_protocol_selection_and_catalog_share_availability() {
        let snapshot = ProviderSnapshot::new([
            (
                Protocol::Gemini,
                "multi".into(),
                model(Protocol::Gemini, "gemini", false, true),
            ),
            (
                Protocol::OpenAiChat,
                "multi".into(),
                model(Protocol::OpenAiChat, "chat", false, true),
            ),
            (
                Protocol::OpenAiResponses,
                "multi".into(),
                model(Protocol::OpenAiResponses, "responses", true, true),
            ),
            (
                Protocol::OpenAiChat,
                "absent".into(),
                model(Protocol::OpenAiChat, "absent", true, true),
            ),
            (
                Protocol::OpenAiChat,
                "disabled".into(),
                model(Protocol::OpenAiChat, "disabled", false, false),
            ),
            (
                Protocol::OpenAiResponses,
                "partial".into(),
                model(Protocol::OpenAiResponses, "off", false, false),
            ),
            (
                Protocol::Gemini,
                "partial".into(),
                model(Protocol::Gemini, "on", false, true),
            ),
        ])
        .unwrap();
        assert_eq!(
            snapshot
                .select(Protocol::Gemini, "multi")
                .unwrap()
                .upstream_model_id,
            "gemini"
        );
        for kind in [Protocol::AnthropicMessages, Protocol::OpenAiResponses] {
            assert_eq!(
                snapshot.select(kind, "multi").unwrap().upstream_model_id,
                "chat"
            );
        }
        assert!(snapshot.select(Protocol::Gemini, "unknown").is_none());
        assert!(snapshot.select(Protocol::Gemini, "absent").unwrap().blocked);
        assert!(
            snapshot
                .select(Protocol::Gemini, "disabled")
                .unwrap()
                .provider
                .is_none()
        );
        assert!(
            snapshot
                .select(Protocol::OpenAiResponses, "partial")
                .unwrap()
                .provider
                .is_none()
        );
        let catalog = snapshot.catalog();
        assert_eq!(
            catalog.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["multi", "partial"]
        );
        assert_eq!(catalog[0].id, catalog[0].name);
        assert_eq!(catalog[0].protocols.len(), 4);
        assert_eq!(
            catalog[0].protocols["gemini"],
            "/v1beta/models/{model}:generateContent"
        );
        assert_eq!(catalog[1].protocols.len(), 3);
        assert!(!catalog[1].protocols.contains_key("responses"));
    }
}
