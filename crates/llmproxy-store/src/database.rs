use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use aes_gcm::aead::Generate;
use base64::{Engine, engine::general_purpose::STANDARD};
use toasty_driver_sqlite::Sqlite;

use crate::{StoreError, StoreResult, crypto::KeyCipher};

pub const DEFAULT_DATABASE_URL: &str = "sqlite:./data/llmproxy.sqlite3";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Backend {
    PostgreSql,
    Sqlite(PathBuf),
}

impl Backend {
    pub fn parse(url: &str) -> StoreResult<Self> {
        if url.starts_with("postgresql://") || url.starts_with("postgres://") {
            return Ok(Self::PostgreSql);
        }
        if url.starts_with("sqlite:") {
            return match Sqlite::new(url) {
                Ok(Sqlite::File(path)) if !path.as_os_str().is_empty() => Ok(Self::Sqlite(path)),
                Ok(Sqlite::InMemory) => Err(StoreError::Configuration(
                    "统一服务须使用持久化 SQLite 文件，不能使用 sqlite::memory:",
                )),
                _ => Err(StoreError::Configuration("LLMPROXY_DATABASE_URL 格式无效")),
            };
        }
        Err(StoreError::Configuration("LLMPROXY_DATABASE_URL 格式无效"))
    }

    pub fn is_sqlite(&self) -> bool {
        matches!(self, Self::Sqlite(_))
    }
}

/// A resolved database and its encryption key. The key is deliberately not Debug.
pub struct DatabaseConfig {
    url: String,
    master_key: String,
    backend: Backend,
}

impl DatabaseConfig {
    pub fn from_env() -> StoreResult<Self> {
        let url = match env::var("LLMPROXY_DATABASE_URL") {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => String::new(),
            Err(env::VarError::NotUnicode(_)) => {
                return Err(StoreError::Configuration(
                    "LLMPROXY_DATABASE_URL 不是有效文本",
                ));
            }
        };
        let key = match env::var("LLMPROXY_MASTER_KEY") {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => String::new(),
            Err(env::VarError::NotUnicode(_)) => {
                return Err(StoreError::Configuration(
                    "LLMPROXY_MASTER_KEY 不是有效文本",
                ));
            }
        };
        Self::from_values(&url, &key)
    }

    pub fn from_values(url: &str, master_key: &str) -> StoreResult<Self> {
        let url = if url.trim().is_empty() {
            DEFAULT_DATABASE_URL
        } else {
            url
        };
        let backend = Backend::parse(url)?;
        let master_key = match &backend {
            Backend::PostgreSql => {
                if master_key.is_empty() {
                    return Err(StoreError::Configuration("LLMPROXY_MASTER_KEY is required"));
                }
                master_key.to_owned()
            }
            Backend::Sqlite(path) => {
                let path = absolute_path(path)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|_| StoreError::Configuration("无法创建 SQLite 数据库目录"))?;
                }
                if master_key.is_empty() {
                    sqlite_key(&path)?
                } else {
                    master_key.to_owned()
                }
            }
        };
        KeyCipher::new(&master_key)?;
        Ok(Self {
            url: url.to_owned(),
            master_key,
            backend,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn master_key(&self) -> &str {
        &self.master_key
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn sqlite_path(&self) -> StoreResult<Option<PathBuf>> {
        match &self.backend {
            Backend::Sqlite(path) => absolute_path(path).map(Some),
            Backend::PostgreSql => Ok(None),
        }
    }
}

fn absolute_path(path: &Path) -> StoreResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|_| StoreError::Configuration("无法解析 SQLite 数据库路径"))
    }
}

fn sqlite_key(database_path: &Path) -> StoreResult<String> {
    let mut name = database_path.as_os_str().to_os_string();
    name.push(".key");
    let key_path = PathBuf::from(name);
    if key_path.exists() {
        return read_key(&key_path);
    }
    if database_path.exists() {
        return Err(StoreError::Configuration(
            "SQLite 数据库已存在但主密钥文件缺失，请恢复配套密钥文件或设置 LLMPROXY_MASTER_KEY",
        ));
    }

    let random = <[u8; 32]>::generate();
    let encoded = STANDARD.encode(random);
    let suffix = u64::generate();
    let mut temp_name = key_path.as_os_str().to_os_string();
    temp_name.push(format!(".tmp-{}-{suffix:016x}", std::process::id()));
    let temp_path = PathBuf::from(temp_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp_path)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        fs::hard_link(&temp_path, &key_path)
    })();
    let _ = fs::remove_file(&temp_path);
    match result {
        Ok(()) => Ok(encoded),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_key(&key_path),
        Err(_) => Err(StoreError::Configuration("无法保存 SQLite 主密钥文件")),
    }
}

fn read_key(path: &Path) -> StoreResult<String> {
    let key = fs::read_to_string(path)
        .map_err(|_| StoreError::Configuration("无法读取 SQLite 主密钥文件"))?;
    let key = key.trim_end_matches(['\r', '\n']).to_owned();
    KeyCipher::new(&key)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn directory() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "llmproxy-database-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn sqlite_key_is_private_stable_and_never_replaced_for_existing_database() {
        let directory = directory();
        let database = directory.join("nested/providers.sqlite3");
        let url = format!("sqlite:{}", database.display());
        let first = DatabaseConfig::from_values(&url, "").unwrap();
        let key_path = directory.join("nested/providers.sqlite3.key");
        assert!(key_path.is_file());
        assert_eq!(first.master_key(), fs::read_to_string(&key_path).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let second = DatabaseConfig::from_values(&url, "").unwrap();
        assert_eq!(first.master_key(), second.master_key());
        fs::write(&database, []).unwrap();
        fs::remove_file(&key_path).unwrap();
        assert!(matches!(
            DatabaseConfig::from_values(&url, ""),
            Err(StoreError::Configuration(_))
        ));
        let explicit = DatabaseConfig::from_values(&url, first.master_key()).unwrap();
        assert_eq!(explicit.master_key(), first.master_key());
        assert!(!key_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn backend_rejects_memory_and_postgresql_requires_key() {
        assert!(matches!(
            Backend::parse(DEFAULT_DATABASE_URL),
            Ok(Backend::Sqlite(_))
        ));
        assert!(Backend::parse("sqlite::memory:").is_err());
        assert!(Backend::parse("sqlite:").is_err());
        assert!(DatabaseConfig::from_values("postgresql://127.0.0.1/db", "").is_err());
        assert!(DatabaseConfig::from_values("mysql://127.0.0.1/db", "").is_err());
    }
}
