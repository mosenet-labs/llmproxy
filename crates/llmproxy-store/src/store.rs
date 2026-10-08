use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use llmproxy_core::{protocol::Protocol, provider::validate_upstream};
use toasty::{Connection, Db, Executor, Transaction};
use toasty_core::driver::operation::TransactionMode;

use crate::{
    ActiveProvider, MessagesAuth, ModelMappingInput, ModelMappingView, ModelPrice,
    ModelProbeTarget, ModelRoute, ModelRouteInput, ModelRouteTargetView, ModelRouteView,
    ProbeStatus, ProviderInput, ProviderView, StoreError, StoreResult,
    crypto::KeyCipher,
    database::Backend,
    model::{
        ChatConversationRow, ChatTurnRow, HolidayDateRow, ModelMapping, ModelPricePlan,
        ModelPriceRule, ModelRouteRow, ModelRouteTargetRow, Provider, RouteBinding, StoreKey,
        ToolContinuationRow, protocol,
    },
    pricing::decimal_price,
};

mod chat_history;
mod holidays;
mod migrations;
mod models;
mod pricing;
mod providers;
mod routes;
mod subscription_nodes;
mod tool_continuations;

use migrations::{MIGRATIONS, SQLITE_MIGRATIONS};
use pricing::backfill_legacy_prices;

const KEY_VERIFIER: &str = "llmproxy.database-master-key.verifier.v1";
const MASTER_KEY_ERROR: StoreError = StoreError::Configuration(
    "数据库主密钥校验失败，请检查 LLMPROXY_MASTER_KEY 或 SQLite 配套密钥文件；不能使用不同主密钥修改此数据库",
);

/// Shared connection pool and credential cipher. Cloning does not reconnect.
#[derive(Clone)]
pub struct ProviderStore {
    db: Db,
    cipher: KeyCipher,
    backend: Backend,
    // SQLite 的同步锁等待不能阻塞持锁事务所在的 async 执行线程。
    chat_writes: Arc<tokio::sync::Mutex<()>>,
    pending_chat_usage: Arc<std::sync::Mutex<HashMap<String, chat_history::PendingUsage>>>,
}

impl ProviderStore {
    /// 统一读取凭据和鉴权配置；路由调用方负责选择协议、路径及启用状态。
    fn active_provider(
        &self,
        provider: &Provider,
        protocol: Protocol,
        upstream_path: String,
    ) -> StoreResult<ActiveProvider> {
        Ok(ActiveProvider {
            name: provider.name.clone(),
            id: provider.id,
            protocol,
            upstream_path,
            host: provider.host.clone(),
            port: provider.port,
            tls: provider.tls,
            secret: self.cipher.decrypt(&provider.encrypted_key)?,
            anthropic_version: provider.anthropic_version.clone(),
            messages_auth: MessagesAuth::parse(&provider.messages_auth)
                .ok_or(StoreError::Internal)?,
            connect_timeout_ms: provider.connect_timeout_ms,
            read_timeout_ms: provider.read_timeout_ms,
            write_timeout_ms: provider.write_timeout_ms,
        })
    }

    /// Connect to an existing database. Schema changes require `migrate`.
    pub async fn connect(url: &str, master_key: &str) -> StoreResult<Self> {
        let backend = Backend::parse(url)?;
        let cipher = KeyCipher::new(master_key)?;
        let db = Db::builder()
            .models(toasty::models!(
                Provider,
                RouteBinding,
                StoreKey,
                ModelMapping,
                ModelRouteRow,
                ModelRouteTargetRow,
                ModelPricePlan,
                ModelPriceRule,
                HolidayDateRow,
                ToolContinuationRow,
                ChatConversationRow,
                ChatTurnRow,
                crate::model::SubscriptionNode
            ))
            .max_pool_size(10)
            .pool_wait_timeout(Some(Duration::from_secs(10)))
            .pool_create_timeout(Some(Duration::from_secs(10)))
            .log_statement_params(false)
            .connect(url)
            .await?;
        if backend.is_sqlite() {
            let mut connection = db.connection().await?;
            toasty::sql::query("PRAGMA journal_mode=WAL")
                .exec(&mut connection)
                .await?;
        }
        Ok(Self {
            db,
            cipher,
            backend,
            chat_writes: Arc::default(),
            pending_chat_usage: Arc::default(),
        })
    }

    pub async fn migrate(&self) -> StoreResult<()> {
        if self.backend.is_sqlite() {
            let Backend::Sqlite(path) = &self.backend else {
                unreachable!()
            };
            let migration_url = format!("sqlite:{}", path.display());
            let migration_db = Db::builder()
                .max_pool_size(1)
                .connect(&migration_url)
                .await?;
            let mut migration_connection = migration_db.connection().await?;
            toasty::sql::query("PRAGMA foreign_keys=OFF")
                .exec(&mut migration_connection)
                .await?;
            drop(migration_connection);
            SQLITE_MIGRATIONS.apply(&migration_db).await?;
            let mut migration_connection = migration_db.connection().await?;
            if !toasty::sql::query("PRAGMA foreign_key_check")
                .exec(&mut migration_connection)
                .await?
                .is_empty()
            {
                return Err(StoreError::Internal);
            }
            toasty::sql::query("PRAGMA foreign_keys=ON")
                .exec(&mut migration_connection)
                .await?;
        }
        let mut connection = self.connection().await?;
        let mut lock = self.transaction(&mut connection, true).await?;
        if !self.backend.is_sqlite() {
            // Serialize PostgreSQL migrations across processes. The lock is
            // transaction scoped, so cancellation cannot strand a pool session.
            toasty::sql::query("SELECT pg_advisory_xact_lock(725076982421)::text")
                .exec(&mut lock)
                .await?;
            MIGRATIONS.apply(&self.db).await?;
        }
        locked_bindings(&mut lock, true, &self.backend).await?;
        if let Some(key) = StoreKey::filter_by_id(1_i64)
            .first()
            .exec(&mut lock)
            .await?
        {
            self.verify(&key)?;
        } else {
            // Upgrading a pre-verifier database must prove the supplied master key
            // can read every existing credential before it can claim this database.
            for provider in Provider::all().exec(&mut lock).await? {
                self.cipher
                    .decrypt(&provider.encrypted_key)
                    .map_err(|_| MASTER_KEY_ERROR)?;
            }
            StoreKey::create()
                .id(1_i64)
                .encrypted_verifier(self.cipher.encrypt(KEY_VERIFIER)?)
                .exec(&mut lock)
                .await?;
        }
        backfill_legacy_prices(&mut lock).await?;
        lock.commit().await?;
        Ok(())
    }

    async fn connection(&self) -> StoreResult<Connection> {
        let mut connection = self.db.connection().await?;
        if self.backend.is_sqlite() {
            // SQLite PRAGMAs are connection local, including after pool growth.
            toasty::sql::statement("PRAGMA foreign_keys=ON")
                .exec(&mut connection)
                .await?;
            toasty::sql::query("PRAGMA busy_timeout=5000")
                .exec(&mut connection)
                .await?;
        }
        Ok(connection)
    }

    async fn transaction<'a>(
        &self,
        connection: &'a mut Connection,
        write: bool,
    ) -> StoreResult<Transaction<'a>> {
        let builder = connection.transaction_builder();
        if self.backend.is_sqlite() && write {
            Ok(builder.mode(TransactionMode::Immediate).begin().await?)
        } else {
            Ok(builder.begin().await?)
        }
    }

    fn verify(&self, key: &StoreKey) -> StoreResult<()> {
        let value = self
            .cipher
            .decrypt(&key.encrypted_verifier)
            .map_err(|_| MASTER_KEY_ERROR)?;
        if value != KEY_VERIFIER {
            return Err(MASTER_KEY_ERROR);
        }
        Ok(())
    }

    async fn bindings(
        &self,
        tx: &mut Transaction<'_>,
        write: bool,
    ) -> StoreResult<Vec<RouteBinding>> {
        let bindings = locked_bindings(tx, write, &self.backend).await?;
        let key = StoreKey::filter_by_id(1_i64)
            .first()
            .exec(tx)
            .await?
            .ok_or(StoreError::Configuration(
                "数据库尚未初始化主密钥，请先运行 llmproxy-db migrate",
            ))?;
        self.verify(&key)?;
        Ok(bindings)
    }
}

/// PostgreSQL locks fixed route rows; SQLite write transactions acquire the
/// database write lock at BEGIN IMMEDIATE. Reads use one transaction snapshot.
async fn locked_bindings(
    tx: &mut Transaction<'_>,
    write: bool,
    backend: &Backend,
) -> StoreResult<Vec<RouteBinding>> {
    let sql = match backend {
        Backend::Sqlite(_) => "SELECT protocol FROM route_bindings ORDER BY protocol",
        Backend::PostgreSql if write => {
            "SELECT protocol FROM route_bindings ORDER BY protocol FOR UPDATE"
        }
        Backend::PostgreSql => "SELECT protocol FROM route_bindings ORDER BY protocol FOR SHARE",
    };
    let rows = toasty::sql::query(sql).exec(tx).await?;
    if rows.len() != 3 {
        return Err(StoreError::Internal);
    }
    Ok(RouteBinding::all().exec(tx).await?)
}

async fn find(executor: &mut dyn Executor, id: i64) -> StoreResult<Provider> {
    Provider::filter_by_id(id)
        .first()
        .exec(executor)
        .await?
        .ok_or(StoreError::NotFound)
}

async fn load_providers(
    executor: &mut dyn Executor,
    ids: impl IntoIterator<Item = i64>,
) -> StoreResult<HashMap<i64, Provider>> {
    let ids: HashSet<_> = ids.into_iter().collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(Provider::all()
        .filter(
            Provider::fields()
                .id()
                .in_list(ids.into_iter().collect::<Vec<_>>()),
        )
        .exec(executor)
        .await?
        .into_iter()
        .map(|provider| (provider.id, provider))
        .collect())
}

async fn find_mapping(executor: &mut dyn Executor, id: i64) -> StoreResult<ModelMapping> {
    ModelMapping::filter_by_id(id)
        .first()
        .exec(executor)
        .await?
        .ok_or(StoreError::NotFound)
}

fn mapping_view(mapping: &ModelMapping, provider: &Provider) -> StoreResult<ModelMappingView> {
    Ok(ModelMappingView {
        thinking: mapping.thinking()?,
        id: mapping.id,
        alias: mapping.alias.clone(),
        provider_id: mapping.provider_id,
        provider_name: provider.name.clone(),
        upstream_model_id: mapping.upstream_model_id.clone(),
        protocols: mapping.protocols(),
        reference_price: mapping
            .input_price_per_million
            .as_ref()
            .zip(mapping.output_price_per_million.as_ref())
            .map(|(input, output)| ModelPrice {
                input_per_million: input.clone(),
                output_per_million: output.clone(),
            }),
        provider_enabled: provider.enabled,
        version: mapping.version,
    })
}

fn validate_mapping(mut input: ModelMappingInput) -> StoreResult<ModelMappingInput> {
    input
        .thinking
        .validate(&input.protocols)
        .map_err(|message| StoreError::Validation(message.into()))?;
    input.alias = input.alias.trim().to_owned();
    if input.alias.is_empty()
        || input.alias.len() > 200
        || input.alias.chars().any(char::is_control)
    {
        return Err(StoreError::Validation("模型标识须为 1–200 个字符".into()));
    }
    if input.upstream_model_id.is_empty()
        || input.upstream_model_id.len() > 200
        || input.upstream_model_id.chars().any(char::is_control)
    {
        return Err(StoreError::Validation("请选择有效的上游模型 ID".into()));
    }
    if input.protocols.is_empty() {
        return Err(StoreError::Validation("请至少选择一个协议".into()));
    }
    if let Some(price) = &input.reference_price {
        for amount in [&price.input_per_million, &price.output_per_million] {
            if decimal_price(amount).is_err() {
                return Err(StoreError::Validation("模型参考价格无效".into()));
            }
        }
    }
    Ok(input)
}

fn check_mapping_provider(provider: &Provider, input: &ModelMappingInput) -> StoreResult<()> {
    if !provider.enabled {
        return Err(StoreError::Conflict("请先启用 Provider".into()));
    }
    if input
        .protocols
        .iter()
        .any(|protocol| provider.paths().get(*protocol).is_none())
    {
        return Err(StoreError::Validation("Provider 未配置所选协议".into()));
    }
    Ok(())
}

async fn check_unique_alias(
    executor: &mut dyn Executor,
    alias: &str,
    own_id: Option<i64>,
) -> StoreResult<()> {
    if let Some(existing) = ModelMapping::filter_by_alias(alias)
        .select(ModelMapping::fields().id())
        .first()
        .exec(executor)
        .await?
        && Some(existing) != own_id
    {
        return Err(StoreError::Conflict("模型标识已存在".into()));
    }
    Ok(())
}

fn check_mapping_version(mapping: &ModelMapping, expected: u64) -> StoreResult<()> {
    if mapping.version != expected {
        return Err(StoreError::Conflict("模型已被修改，请刷新后重试".into()));
    }
    Ok(())
}

async fn check_unique_name(
    executor: &mut dyn Executor,
    name: &str,
    own_id: Option<i64>,
) -> StoreResult<()> {
    if let Some(existing) = Provider::filter_by_name(name)
        .select(Provider::fields().id())
        .first()
        .exec(executor)
        .await?
        && Some(existing) != own_id
    {
        return Err(StoreError::Conflict(
            "已存在同名 Provider，请使用其他名称".into(),
        ));
    }
    Ok(())
}

fn check_version(provider: &Provider, expected: u64) -> StoreResult<()> {
    if provider.version != expected {
        return Err(StoreError::Conflict(
            "Provider 已被其他操作修改，请刷新后重试".into(),
        ));
    }
    Ok(())
}

fn active_protocols(bindings: &[RouteBinding], id: i64) -> Vec<Protocol> {
    bindings
        .iter()
        .filter(|binding| binding.provider_id == Some(id))
        .filter_map(|binding| protocol(&binding.protocol).ok())
        .collect()
}

async fn unbind(
    tx: &mut Transaction<'_>,
    bindings: &mut [RouteBinding],
    id: i64,
) -> StoreResult<()> {
    for binding in bindings.iter_mut().filter(|b| b.provider_id == Some(id)) {
        binding.update().provider_id(None::<i64>).exec(tx).await?;
    }
    Ok(())
}

fn now() -> StoreResult<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StoreError::Internal)?
        .as_secs()
        .try_into()
        .map_err(|_| StoreError::Internal)
}

fn validate(mut input: ProviderInput, creating: bool) -> StoreResult<ProviderInput> {
    input.name = input.name.trim().to_owned();
    if input.name.is_empty()
        || input.name.chars().count() > 80
        || input.name.chars().any(char::is_control)
    {
        return Err(StoreError::Validation(
            "名称须为 1–80 个字符，不能包含控制字符".into(),
        ));
    }
    if input.host.len() > 253
        || !input
            .host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(StoreError::Validation(
            "Host 须为域名或 IPv4 地址，不能包含协议、端口或路径".into(),
        ));
    }
    if input.paths.supported().is_empty() {
        return Err(StoreError::Validation("请至少配置一个接口协议".into()));
    }
    if input.paths.get(input.models_protocol).is_none() {
        return Err(StoreError::Validation(
            "模型探测协议必须是已选择的接口协议".into(),
        ));
    }
    for path in [
        input.paths.openai_chat.as_deref(),
        input.paths.openai_responses.as_deref(),
        input.paths.anthropic_messages.as_deref(),
        input.paths.gemini.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_path(path)?;
    }
    validate_path(&input.models_path)?;
    if (creating && input.api_key.is_empty())
        || (!input.api_key.is_empty()
            && (input.api_key.trim() != input.api_key
                || input.api_key.len() > 8192
                || !input
                    .api_key
                    .bytes()
                    .all(|b| b.is_ascii_graphic() || b == b' ')))
    {
        return Err(StoreError::Validation(
            "API Key 不能为空，不能包含控制字符或首尾空白，且须为可打印 ASCII".into(),
        ));
    }
    if [
        input.connect_timeout_ms,
        input.read_timeout_ms,
        input.write_timeout_ms,
    ]
    .iter()
    .any(|timeout| *timeout > i64::MAX as u64)
    {
        return Err(StoreError::Validation("超时值超出支持范围".into()));
    }
    validate_upstream(
        &input.host,
        input.port,
        input.anthropic_version.as_deref(),
        input.connect_timeout_ms,
        input.read_timeout_ms,
        input.write_timeout_ms,
    )
    .map_err(|message| {
        StoreError::Validation(match message {
            "port must be nonzero" => "端口须为 1–65535".into(),
            "connect_timeout_ms must be nonzero"
            | "read_timeout_ms must be nonzero"
            | "write_timeout_ms must be nonzero" => "连接、读取和写入超时均须大于 0".into(),
            "anthropic_version must be a nonempty printable ASCII header value" => {
                "Anthropic 版本须为非空、可打印 ASCII，不能包含控制字符".into()
            }
            _ => "Host 须为域名或 IPv4 地址，不能包含协议、端口或路径".into(),
        })
    })?;
    Ok(input)
}

fn validate_path(path: &str) -> StoreResult<()> {
    let valid = path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 2048
        && !path.contains(['?', '#', '\\'])
        && !path.chars().any(char::is_whitespace)
        && !path.chars().any(char::is_control)
        && !path
            .split('/')
            .any(|segment| segment == "." || segment == "..");
    if valid {
        Ok(())
    } else {
        Err(StoreError::Validation(
            "接口路径须以 / 开头，不能包含域名、查询参数、片段或相对路径".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::time::{SystemTime, UNIX_EPOCH};
    use toasty::migration::{MigrationFile, MigrationSet};

    static LEGACY_SQLITE: MigrationSet = MigrationSet::new(&[
        MigrationFile::new(
            202609240001,
            "0001_providers.sql",
            include_str!("../migrations/sqlite/0001_providers.sql"),
        ),
        MigrationFile::new(
            202609240002,
            "0002_route_bindings.sql",
            include_str!("../migrations/sqlite/0002_route_bindings.sql"),
        ),
        MigrationFile::new(
            202609240003,
            "0003_seed_bindings.sql",
            include_str!("../migrations/sqlite/0003_seed_bindings.sql"),
        ),
        MigrationFile::new(
            202609240004,
            "0004_store_key.sql",
            include_str!("../migrations/sqlite/0004_store_key.sql"),
        ),
    ]);
    static LEGACY_POSTGRESQL: MigrationSet = MigrationSet::new(&[
        MigrationFile::new(
            202609240001,
            "0001_providers.sql",
            include_str!("../migrations/postgresql/0001_providers.sql"),
        ),
        MigrationFile::new(
            202609240002,
            "0002_store_key.sql",
            include_str!("../migrations/postgresql/0002_store_key.sql"),
        ),
    ]);

    #[tokio::test]
    async fn sqlite_gemini_upgrade_preserves_existing_model_routes() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-gemini-upgrade-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
        let store = ProviderStore::connect(&url, &STANDARD.encode([22; 32]))
            .await
            .unwrap();
        let mut conn = store.db.connection().await.unwrap();
        toasty::sql::query("CREATE TABLE __toasty_migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)").exec(&mut conn).await.unwrap();
        for migration in SQLITE_MIGRATIONS.migrations().iter().take(41) {
            toasty::sql::query(migration.sql())
                .exec(&mut conn)
                .await
                .unwrap();
            toasty::sql::query(format!(
                "INSERT INTO __toasty_migrations VALUES ({}, '{}', '2026-09-29')",
                migration.id(),
                migration.name()
            ))
            .exec(&mut conn)
            .await
            .unwrap();
        }
        let secret = store.cipher.encrypt("legacy-secret").unwrap();
        toasty::sql::query(format!("INSERT INTO providers (id, name, host, port, tls, encrypted_key, enabled, connect_timeout_ms, read_timeout_ms, write_timeout_ms, updated_at, openai_chat_path) VALUES (1, 'Legacy', 'example.com', 443, 1, '{secret}', 1, 1000, 1000, 1000, 1, '/v1/chat/completions')")).exec(&mut conn).await.unwrap();
        toasty::sql::query("INSERT INTO model_mappings (id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, updated_at) VALUES (1, 'legacy-model', 1, 'old-id', 1, 0, 0, 1)").exec(&mut conn).await.unwrap();
        toasty::sql::query("INSERT INTO model_routes (id, name, protocol, updated_at) VALUES (1, 'legacy-route', 'openai_chat', 1)").exec(&mut conn).await.unwrap();
        toasty::sql::query("INSERT INTO model_route_targets (id, route_id, model_id, position) VALUES (1, 1, 1, 1)").exec(&mut conn).await.unwrap();
        drop(conn);
        store.migrate().await.unwrap();
        assert_eq!(
            store.get_model(1).await.unwrap().upstream_model_id,
            "old-id"
        );
        assert_eq!(
            store
                .load_model_routes()
                .await
                .unwrap()
                .iter()
                .find(|route| route.alias == "legacy-route")
                .unwrap()
                .upstream_model_id,
            "old-id"
        );
        let provider = store
            .create(ProviderInput {
                name: "Google".into(),
                paths: crate::ProviderPaths::single(Protocol::Gemini),
                host: "generativelanguage.googleapis.com".into(),
                port: 443,
                tls: true,
                api_key: "gemini-secret".into(),
                enabled: true,
                models_path: "/v1beta/models".into(),
                models_protocol: Protocol::Gemini,
                anthropic_version: None,
                messages_auth: MessagesAuth::ApiKey,
                connect_timeout_ms: 1000,
                read_timeout_ms: 1000,
                write_timeout_ms: 1000,
            })
            .await
            .unwrap();
        store
            .create_model(ModelMappingInput {
                thinking: Default::default(),
                alias: "gemini-model".into(),
                provider_id: provider.id,
                upstream_model_id: "gemini-test".into(),
                protocols: vec![Protocol::Gemini],
                reference_price: None,
            })
            .await
            .unwrap();
        assert!(
            store
                .load_model_routes()
                .await
                .unwrap()
                .iter()
                .any(|route| route.alias == "gemini-model" && route.protocol == Protocol::Gemini)
        );
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn sqlite_model_mapping_enforces_provider_protocols_and_references() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-model-mapping-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
        let store = ProviderStore::connect(&url, &STANDARD.encode([8; 32]))
            .await
            .unwrap();
        store.migrate().await.unwrap();
        let provider_input = ProviderInput {
            name: "Primary".into(),
            paths: crate::ProviderPaths {
                openai_chat: Some("/chat".into()),
                openai_responses: Some("/responses".into()),
                anthropic_messages: None,
                gemini: None,
            },
            host: "api.example.com".into(),
            port: 443,
            tls: true,
            api_key: "test-secret".into(),
            enabled: true,
            models_path: "/models".into(),
            models_protocol: Protocol::OpenAiChat,
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
        };
        let provider = store.create(provider_input.clone()).await.unwrap();
        let mapping = ModelMappingInput {
            thinking: Default::default(),
            alias: "Primary/model".into(),
            provider_id: provider.id,
            upstream_model_id: "upstream-model".into(),
            protocols: vec![Protocol::OpenAiChat, Protocol::OpenAiResponses],
            reference_price: Some(ModelPrice {
                input_per_million: "0.15".into(),
                output_per_million: "0.60".into(),
            }),
        };
        let saved = store.create_model(mapping.clone()).await.unwrap();
        let price = store
            .get_model(saved.id)
            .await
            .unwrap()
            .reference_price
            .unwrap();
        assert_eq!(price.input_per_million, "0.15");
        assert_eq!(price.output_per_million, "0.60");
        assert_eq!(store.list_models().await.unwrap().len(), 1);
        assert!(store.list_routes().await.unwrap().is_empty());
        assert!(
            store
                .load_model_routes()
                .await
                .unwrap()
                .iter()
                .any(|route| {
                    route.alias == "Primary/model"
                        && route.protocol == Protocol::OpenAiChat
                        && route.upstream_model_id == "upstream-model"
                })
        );
        assert!(matches!(
            store
                .create_route(ModelRouteInput {
                    name: "Primary/model".into(),
                    protocol: Protocol::OpenAiChat,
                    provider_protocol: Protocol::OpenAiChat,
                    enabled: true,
                    targets: vec![crate::ModelRouteTargetInput {
                        model_id: saved.id,
                        enabled: true,
                    }],
                })
                .await,
            Err(StoreError::Conflict(_))
        ));
        let route = store
            .create_route(ModelRouteInput {
                name: "public-model".into(),
                protocol: Protocol::OpenAiChat,
                provider_protocol: Protocol::OpenAiChat,
                enabled: true,
                targets: vec![crate::ModelRouteTargetInput {
                    model_id: saved.id,
                    enabled: true,
                }],
            })
            .await
            .unwrap();
        let responses_route = store
            .create_route(ModelRouteInput {
                name: "public-model".into(),
                protocol: Protocol::OpenAiResponses,
                provider_protocol: Protocol::OpenAiResponses,
                enabled: true,
                targets: vec![crate::ModelRouteTargetInput {
                    model_id: saved.id,
                    enabled: true,
                }],
            })
            .await
            .unwrap();
        assert_eq!(store.load_model_routes().await.unwrap().len(), 4);
        let cross_route = store
            .create_route(ModelRouteInput {
                name: "cross-model".into(),
                protocol: Protocol::AnthropicMessages,
                provider_protocol: Protocol::OpenAiChat,
                enabled: true,
                targets: vec![crate::ModelRouteTargetInput {
                    model_id: saved.id,
                    enabled: true,
                }],
            })
            .await
            .unwrap();
        assert_eq!(cross_route.provider_protocol, Protocol::OpenAiChat);
        let selected = store
            .load_model_routes()
            .await
            .unwrap()
            .into_iter()
            .find(|route| route.alias == "cross-model")
            .unwrap();
        assert_eq!(selected.protocol, Protocol::AnthropicMessages);
        assert_eq!(selected.provider.unwrap().protocol, Protocol::OpenAiChat);
        store
            .delete_route(cross_route.id, cross_route.version)
            .await
            .unwrap();
        assert!(matches!(
            store
                .create_route(ModelRouteInput {
                    name: "public-model".into(),
                    protocol: Protocol::OpenAiChat,
                    provider_protocol: Protocol::OpenAiChat,
                    enabled: true,
                    targets: vec![crate::ModelRouteTargetInput {
                        model_id: saved.id,
                        enabled: true
                    }],
                })
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store
                .create_route(ModelRouteInput {
                    name: "wrong-protocol".into(),
                    protocol: Protocol::AnthropicMessages,
                    provider_protocol: Protocol::AnthropicMessages,
                    enabled: true,
                    targets: vec![crate::ModelRouteTargetInput {
                        model_id: saved.id,
                        enabled: true
                    }],
                })
                .await,
            Err(StoreError::Validation(_))
        ));
        let mut chat_only = mapping.clone();
        chat_only.protocols = vec![Protocol::OpenAiChat];
        assert!(matches!(
            store.update_model(saved.id, saved.version, chat_only).await,
            Err(StoreError::Conflict(_))
        ));
        let batch = ["first", "second"].map(|id| ModelMappingInput {
            thinking: Default::default(),
            alias: format!("Primary/{id}"),
            provider_id: provider.id,
            upstream_model_id: id.into(),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
        });
        let imported = store.create_models(batch.to_vec()).await.unwrap();
        assert_eq!(imported.len(), 2);
        assert_eq!(store.list_models().await.unwrap().len(), 3);
        let mut invalid_batch = batch.to_vec();
        invalid_batch[0].alias = "Primary/third".into();
        invalid_batch[0].upstream_model_id = "third".into();
        invalid_batch[1].protocols = vec![Protocol::AnthropicMessages];
        assert!(matches!(
            store.create_models(invalid_batch).await,
            Err(StoreError::Validation(_))
        ));
        assert_eq!(store.list_models().await.unwrap().len(), 3);
        assert!(matches!(
            store.create_model(mapping.clone()).await,
            Err(StoreError::Conflict(_))
        ));
        let mut invalid = mapping.clone();
        invalid.alias = "another".into();
        invalid.protocols = vec![Protocol::AnthropicMessages];
        assert!(matches!(
            store.create_model(invalid).await,
            Err(StoreError::Validation(_))
        ));
        let mut without_chat = provider_input.clone();
        without_chat.paths.openai_chat = None;
        without_chat.models_protocol = Protocol::OpenAiResponses;
        without_chat.api_key.clear();
        assert!(matches!(
            store
                .update(provider.id, provider.version, without_chat)
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store.delete(provider.id, provider.version).await,
            Err(StoreError::Conflict(_))
        ));
        let mut secondary_input = provider_input.clone();
        secondary_input.name = "Secondary".into();
        let secondary = store.create(secondary_input).await.unwrap();
        let secondary_model = store
            .create_model(ModelMappingInput {
                thinking: Default::default(),
                alias: "Secondary/model".into(),
                provider_id: secondary.id,
                upstream_model_id: "backup-model".into(),
                protocols: vec![Protocol::OpenAiChat, Protocol::OpenAiResponses],
                reference_price: None,
            })
            .await
            .unwrap();
        let route = store
            .update_route(
                route.id,
                route.version,
                ModelRouteInput {
                    name: "public-model".into(),
                    protocol: Protocol::OpenAiChat,
                    provider_protocol: Protocol::OpenAiChat,
                    enabled: true,
                    targets: vec![
                        crate::ModelRouteTargetInput {
                            model_id: saved.id,
                            enabled: true,
                        },
                        crate::ModelRouteTargetInput {
                            model_id: secondary_model.id,
                            enabled: true,
                        },
                    ],
                },
            )
            .await
            .unwrap();
        let routes = store.load_model_routes().await.unwrap();
        let selected = routes
            .iter()
            .find(|route| route.alias == "public-model" && route.protocol == Protocol::OpenAiChat)
            .unwrap();
        assert_eq!(selected.upstream_model_id, "upstream-model");
        store
            .set_enabled(provider.id, provider.version, false)
            .await
            .unwrap();
        let routes = store.load_model_routes().await.unwrap();
        let selected = routes
            .iter()
            .find(|route| route.alias == "public-model" && route.protocol == Protocol::OpenAiChat)
            .unwrap();
        assert_eq!(selected.upstream_model_id, "backup-model");
        assert_eq!(selected.provider.as_ref().unwrap().id, secondary.id);
        store
            .set_enabled(secondary.id, secondary.version, false)
            .await
            .unwrap();
        let routes = store.load_model_routes().await.unwrap();
        let selected = routes
            .iter()
            .find(|route| route.alias == "public-model" && route.protocol == Protocol::OpenAiChat)
            .unwrap();
        assert!(selected.provider.is_none());
        assert!(matches!(
            store
                .update_route(
                    route.id,
                    route.version - 1,
                    ModelRouteInput {
                        name: "public-model".into(),
                        protocol: Protocol::OpenAiChat,
                        provider_protocol: Protocol::OpenAiChat,
                        enabled: true,
                        targets: vec![crate::ModelRouteTargetInput {
                            model_id: saved.id,
                            enabled: true
                        }],
                    }
                )
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store.delete_model(saved.id, saved.version).await,
            Err(StoreError::Conflict(_))
        ));
        store.delete_route(route.id, route.version).await.unwrap();
        store
            .delete_route(responses_route.id, responses_route.version)
            .await
            .unwrap();
        store.delete_model(saved.id, saved.version).await.unwrap();
        store
            .delete_model(secondary_model.id, secondary_model.version)
            .await
            .unwrap();
        for model in imported {
            store.delete_model(model.id, model.version).await.unwrap();
        }
        assert!(store.list_models().await.unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn sqlite_pool_connections_enable_foreign_keys_busy_timeout_and_wal() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-sqlite-pragmas-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
        let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
            .await
            .unwrap();
        store.migrate().await.unwrap();
        let mut first = store.connection().await.unwrap();
        let mut second = store.connection().await.unwrap();
        for connection in [&mut first, &mut second] {
            assert!(
                toasty::sql::statement(
                    "UPDATE route_bindings SET provider_id=999999 WHERE protocol='openai_chat'"
                )
                .exec(connection)
                .await
                .is_err()
            );
            let busy_timeout = toasty::sql::query("PRAGMA busy_timeout")
                .exec(connection)
                .await
                .unwrap();
            assert!(format!("{busy_timeout:?}").contains("5000"));
            let journal_mode = toasty::sql::query("PRAGMA journal_mode")
                .exec(connection)
                .await
                .unwrap();
            assert!(format!("{journal_mode:?}").to_lowercase().contains("wal"));
        }
        drop((first, second, store));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn sqlite_probe_preview_and_status_follow_saved_configuration() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-probe-status-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
        let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
            .await
            .unwrap();
        store.migrate().await.unwrap();
        let input = ProviderInput {
            name: "Probe".into(),
            paths: crate::ProviderPaths::single(Protocol::OpenAiChat),
            host: "api.example.com".into(),
            port: 443,
            tls: true,
            api_key: "probe-secret".into(),
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
        assert_eq!(provider.models_probe_status, ProbeStatus::Unprobed);
        let mut draft = input.clone();
        draft.api_key.clear();
        draft.models_path = "/draft/models".into();
        let target = store
            .preview_target(Some(provider.id), draft.clone())
            .await
            .unwrap();
        assert_eq!(target.path, "/draft/models");
        assert_eq!(target.secret, "probe-secret");
        assert_eq!(store.get(provider.id).await.unwrap().models_path, "/models");
        let mut previewed = input.clone();
        previewed.api_key.clear();
        let saved = store
            .update_with_probe_status(
                provider.id,
                provider.version,
                previewed,
                Some(ProbeStatus::Success),
            )
            .await
            .unwrap();
        assert_eq!(saved.models_probe_status, ProbeStatus::Success);
        let mut renamed = input.clone();
        renamed.name = "Renamed".into();
        renamed.api_key.clear();
        let saved = store
            .update(provider.id, saved.version, renamed)
            .await
            .unwrap();
        assert_eq!(saved.models_probe_status, ProbeStatus::Success);
        let saved = store
            .update(provider.id, saved.version, draft)
            .await
            .unwrap();
        assert_eq!(saved.models_probe_status, ProbeStatus::Unprobed);
        assert_eq!(saved.models_path, "/draft/models");
        let mut failed = input;
        failed.api_key.clear();
        failed.models_path = "/draft/models".into();
        store
            .update_with_probe_status(
                provider.id,
                saved.version,
                failed,
                Some(ProbeStatus::Failure),
            )
            .await
            .unwrap();
        assert_eq!(
            store.get(provider.id).await.unwrap().models_probe_status,
            ProbeStatus::Failure
        );
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn sqlite_upgrade_preserves_legacy_provider_and_binding() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-sqlite-upgrade-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
        let key = STANDARD.encode([7; 32]);
        let mut db = Db::builder().connect(&url).await.unwrap();
        LEGACY_SQLITE.apply(&db).await.unwrap();
        let ciphertext = KeyCipher::new(&key)
            .unwrap()
            .encrypt("legacy-secret")
            .unwrap();
        toasty::sql::statement(format!("INSERT INTO providers (name, protocol, host, port, tls, encrypted_key, enabled, connect_timeout_ms, read_timeout_ms, write_timeout_ms, updated_at) VALUES ('Legacy', 'openai_responses', 'api.example.com', 443, 1, '{ciphertext}', 1, 1000, 1000, 1000, 1)"))
            .exec(&mut db).await.unwrap();
        toasty::sql::statement(
            "UPDATE route_bindings SET provider_id=1 WHERE protocol='openai_responses'",
        )
        .exec(&mut db)
        .await
        .unwrap();
        drop(db);
        let store = ProviderStore::connect(&url, &key).await.unwrap();
        store.migrate().await.unwrap();
        let provider = store.get(1).await.unwrap();
        assert_eq!(
            provider.paths.openai_responses.as_deref(),
            Some("/v1/responses")
        );
        assert!(provider.paths.openai_chat.is_none());
        assert_eq!(provider.models_path, "/models");
        assert_eq!(provider.models_protocol, Protocol::OpenAiResponses);
        assert_eq!(provider.models_probe_status, ProbeStatus::Unprobed);
        assert_eq!(provider.active_protocols, vec![Protocol::OpenAiResponses]);
        let active = store.load_active().await.unwrap();
        assert_eq!(active[0].secret, "legacy-secret");
        assert_eq!(active[0].upstream_path, "/v1/responses");
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn postgresql_upgrade_preserves_legacy_provider_and_binding() {
        let Ok(base_url) = std::env::var("LLMPROXY_TEST_DATABASE_URL") else {
            return;
        };
        let mut admin = Db::builder().connect(&base_url).await.unwrap();
        let schema = format!(
            "llmproxy_upgrade_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        toasty::sql::statement(format!("CREATE SCHEMA {schema}"))
            .exec(&mut admin)
            .await
            .unwrap();
        let separator = if base_url.contains('?') { '&' } else { '?' };
        let url = format!("{base_url}{separator}options=-c%20search_path%3D{schema}");
        let worker = tokio::spawn(async move {
            let key = STANDARD.encode([7; 32]);
            let mut db = Db::builder().connect(&url).await.unwrap();
            LEGACY_POSTGRESQL.apply(&db).await.unwrap();
            let ciphertext = KeyCipher::new(&key)
                .unwrap()
                .encrypt("legacy-secret")
                .unwrap();
            toasty::sql::statement(format!("INSERT INTO providers (name, protocol, host, port, tls, encrypted_key, enabled, connect_timeout_ms, read_timeout_ms, write_timeout_ms, updated_at) VALUES ('Legacy', 'anthropic_messages', 'api.example.com', 443, TRUE, '{ciphertext}', TRUE, 1000, 1000, 1000, 1)"))
                .exec(&mut db).await.unwrap();
            toasty::sql::statement(
                "UPDATE route_bindings SET provider_id=1 WHERE protocol='anthropic_messages'",
            )
            .exec(&mut db)
            .await
            .unwrap();
            drop(db);
            let store = ProviderStore::connect(&url, &key).await.unwrap();
            store.migrate().await.unwrap();
            let provider = store.get(1).await.unwrap();
            assert_eq!(
                provider.paths.anthropic_messages.as_deref(),
                Some("/v1/messages")
            );
            assert_eq!(provider.models_protocol, Protocol::AnthropicMessages);
            assert_eq!(provider.active_protocols, vec![Protocol::AnthropicMessages]);
            assert_eq!(
                store.load_active().await.unwrap()[0].secret,
                "legacy-secret"
            );
        });
        let result = worker.await;
        toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
            .exec(&mut admin)
            .await
            .unwrap();
        result.unwrap();
    }
}
