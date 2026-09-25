use std::time::{Duration, SystemTime, UNIX_EPOCH};

use llmproxy_core::{protocol::Protocol, provider::validate_upstream};
use toasty::{
    Connection, Db, Executor, Transaction,
    migration::{MigrationFile, MigrationSet},
};
use toasty_core::driver::operation::TransactionMode;

use crate::{
    ActiveProvider, ModelProbeTarget, ProbeStatus, ProviderInput, ProviderView, StoreError,
    StoreResult,
    crypto::KeyCipher,
    database::Backend,
    model::{Provider, RouteBinding, StoreKey, protocol},
};

static MIGRATIONS: MigrationSet = MigrationSet::new(&[
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
    MigrationFile::new(
        202609250001,
        "0003_provider_interfaces.sql",
        include_str!("../migrations/postgresql/0003_provider_interfaces.sql"),
    ),
    MigrationFile::new(
        202609250002,
        "0004_model_probe_protocol_status.sql",
        include_str!("../migrations/postgresql/0004_model_probe_protocol_status.sql"),
    ),
]);

static SQLITE_MIGRATIONS: MigrationSet = MigrationSet::new(&[
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
    MigrationFile::new(
        202609250001,
        "0005_openai_chat_path.sql",
        include_str!("../migrations/sqlite/0005_openai_chat_path.sql"),
    ),
    MigrationFile::new(
        202609250002,
        "0006_openai_responses_path.sql",
        include_str!("../migrations/sqlite/0006_openai_responses_path.sql"),
    ),
    MigrationFile::new(
        202609250003,
        "0007_anthropic_messages_path.sql",
        include_str!("../migrations/sqlite/0007_anthropic_messages_path.sql"),
    ),
    MigrationFile::new(
        202609250004,
        "0008_models_path.sql",
        include_str!("../migrations/sqlite/0008_models_path.sql"),
    ),
    MigrationFile::new(
        202609250005,
        "0009_models_auth.sql",
        include_str!("../migrations/sqlite/0009_models_auth.sql"),
    ),
    MigrationFile::new(
        202609250006,
        "0010_backfill_provider_interfaces.sql",
        include_str!("../migrations/sqlite/0010_backfill_provider_interfaces.sql"),
    ),
    MigrationFile::new(
        202609250007,
        "0011_drop_provider_protocol.sql",
        include_str!("../migrations/sqlite/0011_drop_provider_protocol.sql"),
    ),
    MigrationFile::new(
        202609250008,
        "0012_models_protocol.sql",
        include_str!("../migrations/sqlite/0012_models_protocol.sql"),
    ),
    MigrationFile::new(
        202609250009,
        "0013_models_probe_status.sql",
        include_str!("../migrations/sqlite/0013_models_probe_status.sql"),
    ),
    MigrationFile::new(
        202609250010,
        "0014_backfill_models_protocol.sql",
        include_str!("../migrations/sqlite/0014_backfill_models_protocol.sql"),
    ),
    MigrationFile::new(
        202609250011,
        "0015_drop_models_auth.sql",
        include_str!("../migrations/sqlite/0015_drop_models_auth.sql"),
    ),
]);

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
}

impl ProviderStore {
    /// Connect to an existing database. Schema changes require `migrate`.
    pub async fn connect(url: &str, master_key: &str) -> StoreResult<Self> {
        let backend = Backend::parse(url)?;
        let cipher = KeyCipher::new(master_key)?;
        let db = Db::builder()
            .models(toasty::models!(Provider, RouteBinding, StoreKey))
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
        })
    }

    pub async fn migrate(&self) -> StoreResult<()> {
        if self.backend.is_sqlite() {
            SQLITE_MIGRATIONS.apply(&self.db).await?;
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

    pub async fn list(&self) -> StoreResult<Vec<ProviderView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let providers = Provider::all()
            .order_by(Provider::fields().id().asc())
            .exec(&mut tx)
            .await?;
        let views = providers
            .iter()
            .map(|p| p.view(active_protocols(&bindings, p.id)))
            .collect();
        tx.commit().await?;
        views
    }

    pub async fn get(&self, id: i64) -> StoreResult<ProviderView> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let provider = find(&mut tx, id).await?;
        let view = provider.view(active_protocols(&bindings, id))?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn create(&self, input: ProviderInput) -> StoreResult<ProviderView> {
        self.create_with_probe_status(input, ProbeStatus::Unprobed)
            .await
    }

    pub async fn create_with_probe_status(
        &self,
        input: ProviderInput,
        status: ProbeStatus,
    ) -> StoreResult<ProviderView> {
        let input = validate(input, true)?;
        let encrypted_key = self.cipher.encrypt(&input.api_key)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        check_unique_name(&mut tx, &input.name, None).await?;
        let provider = Provider::create()
            .name(input.name)
            .openai_chat_path(input.paths.openai_chat)
            .openai_responses_path(input.paths.openai_responses)
            .anthropic_messages_path(input.paths.anthropic_messages)
            .host(input.host)
            .port(input.port)
            .tls(input.tls)
            .encrypted_key(encrypted_key)
            .enabled(input.enabled)
            .models_path(input.models_path)
            .models_protocol(input.models_protocol.as_str())
            .models_probe_status(status.as_str())
            .anthropic_version(input.anthropic_version)
            .connect_timeout_ms(input.connect_timeout_ms)
            .read_timeout_ms(input.read_timeout_ms)
            .write_timeout_ms(input.write_timeout_ms)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        let view = provider.view(Vec::new())?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn update(
        &self,
        id: i64,
        version: u64,
        input: ProviderInput,
    ) -> StoreResult<ProviderView> {
        self.update_with_probe_status(id, version, input, None)
            .await
    }

    pub async fn update_with_probe_status(
        &self,
        id: i64,
        version: u64,
        input: ProviderInput,
        status: Option<ProbeStatus>,
    ) -> StoreResult<ProviderView> {
        let input = validate(input, false)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        check_unique_name(&mut tx, &input.name, Some(id)).await?;
        if input.enabled
            && active_protocols(&bindings, id)
                .into_iter()
                .any(|protocol| input.paths.get(protocol).is_none())
        {
            return Err(StoreError::Conflict(
                "当前路由正在使用此 Provider，请先停用或切换后再移除协议".into(),
            ));
        }
        let encrypted_key = self
            .cipher
            .replacement(&input.api_key, &provider.encrypted_key)?;
        let probe_changed = input.host != provider.host
            || input.port != provider.port
            || input.tls != provider.tls
            || input.paths != provider.paths()
            || input.models_path != provider.models_path
            || input.models_protocol.as_str() != provider.models_protocol
            || (!input.api_key.is_empty())
            || input.anthropic_version != provider.anthropic_version;
        let probe_status = status
            .map(|status| status.as_str().to_owned())
            .unwrap_or_else(|| {
                if probe_changed {
                    ProbeStatus::Unprobed.as_str().to_owned()
                } else {
                    provider.models_probe_status.clone()
                }
            });
        provider
            .update()
            .name(input.name)
            .openai_chat_path(input.paths.openai_chat)
            .openai_responses_path(input.paths.openai_responses)
            .anthropic_messages_path(input.paths.anthropic_messages)
            .host(input.host)
            .port(input.port)
            .tls(input.tls)
            .encrypted_key(encrypted_key)
            .enabled(input.enabled)
            .models_path(input.models_path)
            .models_protocol(input.models_protocol.as_str())
            .models_probe_status(probe_status)
            .anthropic_version(input.anthropic_version)
            .connect_timeout_ms(input.connect_timeout_ms)
            .read_timeout_ms(input.read_timeout_ms)
            .write_timeout_ms(input.write_timeout_ms)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        if !provider.enabled {
            unbind(&mut tx, &mut bindings, id).await?;
        }
        let view = provider.view(active_protocols(&bindings, id))?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn set_enabled(&self, id: i64, version: u64, enabled: bool) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        provider
            .update()
            .enabled(enabled)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        if !enabled {
            unbind(&mut tx, &mut bindings, id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn activate(&self, id: i64, version: u64, protocol: Protocol) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        if !provider.enabled {
            return Err(StoreError::Conflict(
                "请先启用此 Provider，再设为当前路由".into(),
            ));
        }
        if provider.paths().get(protocol).is_none() {
            return Err(StoreError::Validation("此 Provider 未配置该协议".into()));
        }
        // Validate decryptability before replacing the previous working binding.
        self.cipher.decrypt(&provider.encrypted_key)?;
        let binding = bindings
            .iter_mut()
            .find(|b| b.protocol == protocol.as_str())
            .ok_or(StoreError::Internal)?;
        if let Some(previous_id) = binding.provider_id.filter(|previous| *previous != id) {
            // Changing another provider's visible active state must invalidate its
            // stale edit form, too.
            let mut previous = find(&mut tx, previous_id).await?;
            previous.update().updated_at(now()?).exec(&mut tx).await?;
        }
        provider.update().updated_at(now()?).exec(&mut tx).await?;
        binding.update().provider_id(Some(id)).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, id: i64, version: u64) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let bindings = self.bindings(&mut tx, true).await?;
        let provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        if is_active(&bindings, id) {
            return Err(StoreError::Conflict(
                "当前路由正在使用此 Provider，请先停用后再删除".into(),
            ));
        }
        if provider.enabled {
            return Err(StoreError::Conflict(
                "Provider 仍处于启用状态，请先停用后再删除".into(),
            ));
        }
        provider.delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_active(&self) -> StoreResult<Vec<ActiveProvider>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let mut active = Vec::with_capacity(3);
        for binding in bindings {
            let Some(id) = binding.provider_id else {
                continue;
            };
            let provider = find(&mut tx, id).await?;
            let protocol = protocol(&binding.protocol)?;
            let Some(upstream_path) = provider.paths().get(protocol).map(str::to_owned) else {
                // Treat inconsistent storage as a failed snapshot, not partial config.
                return Err(StoreError::Internal);
            };
            if !provider.enabled {
                return Err(StoreError::Internal);
            }
            active.push(ActiveProvider {
                id,
                protocol,
                upstream_path,
                host: provider.host,
                port: provider.port,
                tls: provider.tls,
                secret: self.cipher.decrypt(&provider.encrypted_key)?,
                anthropic_version: provider.anthropic_version,
                connect_timeout_ms: provider.connect_timeout_ms,
                read_timeout_ms: provider.read_timeout_ms,
                write_timeout_ms: provider.write_timeout_ms,
            });
        }
        tx.commit().await?;
        Ok(active)
    }

    pub async fn probe_target(&self, id: i64) -> StoreResult<ModelProbeTarget> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let provider = find(&mut tx, id).await?;
        let target = ModelProbeTarget {
            host: provider.host,
            port: provider.port,
            tls: provider.tls,
            path: provider.models_path,
            protocol: protocol(&provider.models_protocol)?,
            secret: self.cipher.decrypt(&provider.encrypted_key)?,
            anthropic_version: provider.anthropic_version,
        };
        tx.commit().await?;
        Ok(target)
    }

    pub async fn preview_target(
        &self,
        id: Option<i64>,
        input: ProviderInput,
    ) -> StoreResult<ModelProbeTarget> {
        let input = validate(input, id.is_none())?;
        let secret = if input.api_key.is_empty() {
            let id = id.ok_or(StoreError::Internal)?;
            self.probe_target(id).await?.secret
        } else {
            input.api_key
        };
        Ok(ModelProbeTarget {
            host: input.host,
            port: input.port,
            tls: input.tls,
            path: input.models_path,
            protocol: input.models_protocol,
            secret,
            anthropic_version: input.anthropic_version,
        })
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

async fn check_unique_name(
    executor: &mut dyn Executor,
    name: &str,
    own_id: Option<i64>,
) -> StoreResult<()> {
    if let Some(existing) = Provider::filter_by_name(name)
        .first()
        .exec(executor)
        .await?
        && Some(existing.id) != own_id
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

fn is_active(bindings: &[RouteBinding], id: i64) -> bool {
    bindings
        .iter()
        .any(|binding| binding.provider_id == Some(id))
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
