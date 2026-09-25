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
use llmproxy_store::{DatabaseConfig, ProviderInput, ProviderStore};

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

fn start(directory: &Path) -> RunningGateway {
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    let log = File::create(directory.join("startup.log")).unwrap();
    let mut process = command(directory);
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
    let first = start(&directory);
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
                protocol: Protocol::OpenAiChat,
                host: "api.example.com".into(),
                port: 443,
                tls: true,
                api_key: "startup-test-key".into(),
                enabled: true,
                anthropic_version: None,
                connect_timeout_ms: 1000,
                read_timeout_ms: 1000,
                write_timeout_ms: 1000,
            })
            .await
            .unwrap();
        store.activate(provider.id, provider.version).await.unwrap();
    });
    drop(runtime);

    let second = start(&directory);
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
