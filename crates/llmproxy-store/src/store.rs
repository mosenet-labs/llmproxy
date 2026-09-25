use std::time::{Duration, SystemTime, UNIX_EPOCH};

use llmproxy_core::provider::validate_upstream;
use toasty::{
    Connection, Db, Executor, Transaction,
    migration::{MigrationFile, MigrationSet},
};
use toasty_core::driver::operation::TransactionMode;

use crate::{
    ActiveProvider, ProviderInput, ProviderView, StoreError, StoreResult,
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
            .map(|p| p.view(is_active(&bindings, p.id)))
            .collect();
        tx.commit().await?;
        views
    }

    pub async fn get(&self, id: i64) -> StoreResult<ProviderView> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let provider = find(&mut tx, id).await?;
        let view = provider.view(is_active(&bindings, id))?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn create(&self, input: ProviderInput) -> StoreResult<ProviderView> {
        let input = validate(input, true)?;
        let encrypted_key = self.cipher.encrypt(&input.api_key)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        check_unique_name(&mut tx, &input.name, None).await?;
        let provider = Provider::create()
            .name(input.name)
            .protocol(input.protocol.as_str())
            .host(input.host)
            .port(input.port)
            .tls(input.tls)
            .encrypted_key(encrypted_key)
            .enabled(input.enabled)
            .anthropic_version(input.anthropic_version)
            .connect_timeout_ms(input.connect_timeout_ms)
            .read_timeout_ms(input.read_timeout_ms)
            .write_timeout_ms(input.write_timeout_ms)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        let view = provider.view(false)?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn update(
        &self,
        id: i64,
        version: u64,
        input: ProviderInput,
    ) -> StoreResult<ProviderView> {
        let input = validate(input, false)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        check_unique_name(&mut tx, &input.name, Some(id)).await?;
        let active = is_active(&bindings, id);
        if active && provider.protocol != input.protocol.as_str() {
            return Err(StoreError::Conflict(
                "当前路由正在使用此 Provider，请先停用或切换后再修改协议".into(),
            ));
        }
        let encrypted_key = self
            .cipher
            .replacement(&input.api_key, &provider.encrypted_key)?;
        provider
            .update()
            .name(input.name)
            .protocol(input.protocol.as_str())
            .host(input.host)
            .port(input.port)
            .tls(input.tls)
            .encrypted_key(encrypted_key)
            .enabled(input.enabled)
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
        let view = provider.view(is_active(&bindings, id))?;
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

    pub async fn activate(&self, id: i64, version: u64) -> StoreResult<()> {
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
        // Validate decryptability before replacing the previous working binding.
        self.cipher.decrypt(&provider.encrypted_key)?;
        let binding = bindings
            .iter_mut()
            .find(|b| b.protocol == provider.protocol)
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
            if !provider.enabled || provider.protocol != binding.protocol {
                // Treat inconsistent storage as a failed snapshot, not partial config.
                return Err(StoreError::Internal);
            }
            active.push(ActiveProvider {
                id,
                protocol: protocol(&provider.protocol)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::time::{SystemTime, UNIX_EPOCH};

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
}
