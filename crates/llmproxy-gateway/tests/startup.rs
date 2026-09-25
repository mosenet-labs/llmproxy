use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime},
};

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{DatabaseConfig, ProviderInput, ProviderPaths, ProviderStore};
use toasty::migration::{MigrationFile, MigrationSet};

const MASTER_KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
const OLD_POSTGRESQL_MIGRATIONS: MigrationSet = MigrationSet::new(&[
    MigrationFile::new(
        202609240001,
        "0001_providers.sql",
        include_str!("../../llmproxy-store/migrations/postgresql/0001_providers.sql"),
    ),
    MigrationFile::new(
        202609240002,
        "0002_store_key.sql",
        include_str!("../../llmproxy-store/migrations/postgresql/0002_store_key.sql"),
    ),
]);

struct RunningGateway {
    child: Child,
    address: SocketAddr,
}

impl Drop for RunningGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "llmproxy-startup-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn command(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_llmproxy"));
    command.env_clear().current_dir(directory);
    command
}

fn request(address: SocketAddr, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(200)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

fn start(directory: &Path, database_url: Option<&str>) -> RunningGateway {
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    let log = File::create(directory.join("startup.log")).unwrap();
    let mut process = command(directory);
    if let Some(database_url) = database_url {
        process
            .env("LLMPROXY_DATABASE_URL", database_url)
            .env("LLMPROXY_MASTER_KEY", MASTER_KEY);
    }
    process
        .env("LLMPROXY_LISTEN", address.to_string())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    drop(reservation);
    let child = process.spawn().unwrap();
    let mut running = RunningGateway { child, address };
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(response) = request(address, "/ui")
            && response.starts_with("HTTP/1.1 200")
        {
            return running;
        }
        if let Some(status) = running.child.try_wait().unwrap() {
            panic!(
                "gateway exited {status}: {}",
                fs::read_to_string(directory.join("startup.log")).unwrap_or_default()
            );
        }
        assert!(Instant::now() < deadline, "gateway did not become ready");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn default_sqlite_starts_and_persists_provider_across_restart() {
    let directory = directory("default");
    let database = directory.join("data/llmproxy.sqlite3");
    let key_file = directory.join("data/llmproxy.sqlite3.key");
    let first = start(&directory, None);
    assert!(database.is_file());
    assert!(key_file.is_file());
    assert!(
        request(first.address, "/ui")
            .unwrap()
            .contains("Provider 管理")
    );
    drop(first);

    let url = format!("sqlite:{}", database.display());
    let config = DatabaseConfig::from_values(&url, "").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = ProviderStore::connect(config.url(), config.master_key())
            .await
            .unwrap();
        let provider = store
            .create(ProviderInput {
                name: "Restarted SQLite Provider".into(),
                paths: ProviderPaths::single(Protocol::OpenAiChat),
                host: "api.example.com".into(),
                port: 443,
                tls: true,
                api_key: "startup-test-key".into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::OpenAiChat,
                anthropic_version: None,
                connect_timeout_ms: 1000,
                read_timeout_ms: 1000,
                write_timeout_ms: 1000,
            })
            .await
            .unwrap();
        store
            .activate(provider.id, provider.version, Protocol::OpenAiChat)
            .await
            .unwrap();
    });
    drop(runtime);

    let second = start(&directory, None);
    let html = request(second.address, "/ui").unwrap();
    assert!(html.contains("Restarted SQLite Provider"));
    drop(second);
    fs::remove_file(&key_file).unwrap();
    let rejected = command(&directory).output().unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("主密钥文件缺失"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_database_errors_do_not_fall_back_to_sqlite() {
    let directory = directory("explicit");
    let no_key = command(&directory)
        .env("LLMPROXY_DATABASE_URL", "postgresql://127.0.0.1:1/unused")
        .output()
        .unwrap();
    assert!(!no_key.status.success());
    assert!(String::from_utf8_lossy(&no_key.stderr).contains("LLMPROXY_MASTER_KEY is required"));
    let unavailable = command(&directory)
        .env("LLMPROXY_DATABASE_URL", "postgresql://127.0.0.1:1/unused")
        .env(
            "LLMPROXY_MASTER_KEY",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        )
        .output()
        .unwrap();
    assert!(!unavailable.status.success());
    let memory = command(&directory)
        .env("LLMPROXY_DATABASE_URL", "sqlite::memory:")
        .output()
        .unwrap();
    assert!(!memory.status.success());
    assert!(String::from_utf8_lossy(&memory.stderr).contains("不能使用 sqlite::memory:"));
    assert!(!directory.join("data").exists());
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postgresql_startup_applies_pending_migration_and_rejects_wrong_key() {
    let Ok(base_url) = std::env::var("LLMPROXY_TEST_DATABASE_URL") else {
        eprintln!("跳过 PostgreSQL 启动迁移测试：未设置 LLMPROXY_TEST_DATABASE_URL");
        return;
    };
    let mut admin = toasty::Db::builder().connect(&base_url).await.unwrap();
    let schema = format!(
        "llmproxy_startup_test_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
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
        let mut db = toasty::Db::builder().connect(&url).await.unwrap();
        OLD_POSTGRESQL_MIGRATIONS.apply(&db).await.unwrap();
        let directory = directory("postgresql-migration");
        let gateway = start(&directory, Some(&url));
        assert_eq!(
            toasty::sql::query("SELECT id FROM __toasty_migrations")
                .exec(&mut db)
                .await
                .unwrap()
                .len(),
            4
        );
        toasty::sql::query("SELECT openai_chat_path FROM providers")
            .exec(&mut db)
            .await
            .unwrap();
        drop(gateway);

        let rejected = command(&directory)
            .env("LLMPROXY_DATABASE_URL", &url)
            .env(
                "LLMPROXY_MASTER_KEY",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("数据库主密钥校验失败"));
        fs::remove_dir_all(directory).unwrap();
    });
    let result = worker.await;
    toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
        .exec(&mut admin)
        .await
        .unwrap();
    result.unwrap();
}
