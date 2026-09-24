use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{ProviderInput, ProviderStore};
use reqwest::{Client, Response, StatusCode, redirect::Policy};

const MASTER_KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
const PROVIDER_KEY: &str = "console-test-key-never-render-this";

struct ConsoleProcess {
    child: Child,
    directory: PathBuf,
}

impl Drop for ConsoleProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Uses an isolated PostgreSQL schema and a temporary child cwd so the developer's
/// .env, records, and running console cannot affect this HTTP acceptance test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_http_crud_and_request_protection() {
    let Ok(base_url) = std::env::var("LLMPROXY_TEST_DATABASE_URL") else {
        eprintln!("跳过控制台 HTTP 集成测试：未设置 LLMPROXY_TEST_DATABASE_URL");
        return;
    };
    let mut admin = toasty::Db::builder()
        .connect(&base_url)
        .await
        .unwrap_or_else(|_| panic!("无法连接 LLMPROXY_TEST_DATABASE_URL"));
    let schema = format!(
        "llmproxy_console_test_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    toasty::sql::statement(format!("CREATE SCHEMA {schema}"))
        .exec(&mut admin)
        .await
        .expect("create isolated schema");
    let separator = if base_url.contains('?') { '&' } else { '?' };
    let url = format!("{base_url}{separator}options=-c%20search_path%3D{schema}");
    let worker = tokio::spawn(async move { exercise_http(&url).await });
    let result = worker.await;
    toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
        .exec(&mut admin)
        .await
        .expect("remove isolated schema");
    result.unwrap();
}

async fn exercise_http(database_url: &str) {
    let store = ProviderStore::connect(database_url, MASTER_KEY)
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let directory =
        std::env::temp_dir().join(format!("llmproxy-console-{}-{port}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_llmproxy-console"))
        .env_clear()
        .env("LLMPROXY_DATABASE_URL", database_url)
        .env(
            "LLMPROXY_MASTER_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .env("LLMPROXY_CONSOLE_PORT", port.to_string())
        .current_dir(&directory)
        .output()
        .expect("start console with a wrong master key");
    assert!(
        !rejected.status.success(),
        "wrong master key must fail before listening"
    );
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains(database_url));
    let child = Command::new(env!("CARGO_BIN_EXE_llmproxy-console"))
        .env_clear()
        .env("LLMPROXY_DATABASE_URL", database_url)
        .env("LLMPROXY_MASTER_KEY", MASTER_KEY)
        .env("LLMPROXY_CONSOLE_PORT", port.to_string())
        .current_dir(&directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start console process");
    let mut process = ConsoleProcess { child, directory };
    let base = format!("http://127.0.0.1:{port}");
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..100 {
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "console exited before binding"
        );
        if client
            .get(&base)
            .send()
            .await
            .is_ok_and(|response| response.status() == StatusCode::OK)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "console failed to become ready");

    let response = client.get(&base).send().await.unwrap();
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let html = response.text().await.unwrap();
    assert!(html.contains("连接第一个模型服务"));
    let alias = client
        .get(format!("{base}/providers"))
        .send()
        .await
        .unwrap();
    assert_eq!(alias.status(), StatusCode::SEE_OTHER);
    assert_eq!(alias.headers()["location"], "/");
    for asset in ["components.css", "console.css", "runtime.js"] {
        let response = client
            .get(format!("{base}/assets/{asset}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{asset}");
        assert!(response.bytes().await.unwrap().len() > 500, "{asset}");
    }

    let editor = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = hidden(&editor, "csrf");
    assert_eq!(csrf.len(), 64);
    assert!(editor.contains("type=\"password\""));
    assert!(editor.contains("name=\"upstream_url\""));
    for field in ["host", "port", "tls"] {
        assert!(!editor.contains(&format!("name=\"{field}\"")));
    }
    let bad_host = client
        .get(&base)
        .header("host", "rebind.invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_host.status(), StatusCode::FORBIDDEN);
    let bad_origin = client
        .post(format!("{base}/providers/save"))
        .header("origin", "https://foreign.invalid")
        .form(&provider_form(&csrf, "Blocked", "openai_chat"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
    let bad_csrf = post(
        &client,
        &base,
        "/providers/save",
        &provider_form("invalid", "Blocked", "openai_chat"),
    )
    .await;
    assert_eq!(bad_csrf.status(), StatusCode::FORBIDDEN);
    let mut missing_csrf = provider_form(&csrf, "Blocked", "openai_chat");
    missing_csrf.retain(|(key, _)| key != "csrf");
    assert_eq!(
        post(&client, &base, "/providers/save", &missing_csrf)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(store.list().await.unwrap().is_empty());

    for upstream_url in [
        "",
        "api.example.com",
        "https://api.example.com/v1",
        "https://api.example.com?debug=true",
        "https://api.example.com#fragment",
        "https://user:password@api.example.com",
        "ftp://api.example.com",
        "http://[::1]:11434",
        "https://api.example.com:65536",
    ] {
        let mut invalid = provider_form(&csrf, "保留这个名称", "openai_chat");
        set(&mut invalid, "upstream_url", upstream_url);
        let invalid_html = post(&client, &base, "/providers/save", &invalid)
            .await
            .text()
            .await
            .unwrap();
        assert!(invalid_html.contains("保存失败"), "accepted {upstream_url}");
        assert!(invalid_html.contains("保留这个名称"));
        assert!(!invalid_html.contains(PROVIDER_KEY));
        assert!(store.list().await.unwrap().is_empty());
    }

    for (name, protocol, upstream_url, host, port, tls) in [
        (
            "Chat Test",
            "openai_chat",
            "https://api.deepseek.com",
            "api.deepseek.com",
            443,
            true,
        ),
        (
            "Responses Test",
            "openai_responses",
            "https://api.example.com:8443/",
            "api.example.com",
            8443,
            true,
        ),
        (
            "Messages Test",
            "anthropic_messages",
            "http://localhost/",
            "localhost",
            80,
            false,
        ),
    ] {
        let mut fields = provider_form(&csrf, name, protocol);
        set(&mut fields, "upstream_url", upstream_url);
        let response = post(&client, &base, "/providers/save", &fields).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/?saved=1");
        let saved = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|provider| provider.name == name)
            .unwrap();
        assert_eq!(
            (saved.host.as_str(), saved.port, saved.tls),
            (host, port, tls)
        );
    }

    // Existing database records still contain separate host/port/TLS fields.
    // The editor reconstructs their URL without requiring a data migration.
    for (port, tls, upstream_url) in [
        (443, true, "https://legacy.example.com"),
        (80, false, "http://legacy.example.com"),
        (8443, true, "https://legacy.example.com:8443"),
    ] {
        let legacy = store
            .create(ProviderInput {
                name: "Legacy Provider".into(),
                protocol: Protocol::OpenAiChat,
                host: "legacy.example.com".into(),
                port,
                tls,
                api_key: PROVIDER_KEY.into(),
                enabled: true,
                anthropic_version: None,
                connect_timeout_ms: 10000,
                read_timeout_ms: 60000,
                write_timeout_ms: 30000,
            })
            .await
            .unwrap();
        let editor = client
            .get(format!("{base}/providers/form?id={}", legacy.id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(hidden(&editor, "upstream_url"), upstream_url);
        assert!(!editor.contains(PROVIDER_KEY));
        store
            .set_enabled(legacy.id, legacy.version, false)
            .await
            .unwrap();
        let disabled = store.get(legacy.id).await.unwrap();
        store.delete(disabled.id, disabled.version).await.unwrap();
    }
    let providers = store.list().await.unwrap();
    assert_eq!(providers.len(), 3);
    assert!(
        providers
            .iter()
            .filter(|provider| provider.name != "Messages Test")
            .all(|provider| provider.anthropic_version.is_none()),
        "non-Anthropic forms must ignore an unrelated version field"
    );
    let chat = providers
        .iter()
        .find(|provider| provider.name == "Chat Test")
        .unwrap();
    let filtered = client
        .get(format!("{base}/?q=Chat&protocol=openai_chat&state=enabled"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(filtered.contains("Chat Test"));
    assert!(!filtered.contains("Responses Test"));
    assert!(!filtered.contains("Messages Test"));
    assert!(!filtered.contains(PROVIDER_KEY));
    assert!(filtered.contains("设为当前服务"));
    assert!(filtered.contains(&format!("id=\"disable-{}\"", chat.id)));
    assert!(!filtered.contains(&format!("id=\"delete-{}\"", chat.id)));
    assert!(filtered.contains("确认停用「Chat Test」？"));
    assert!(filtered.contains("确认停用"));

    // Hiding deletion in the UI must also be enforced for a direct POST, even
    // when the enabled provider has not been selected for the current route.
    let rejected_delete = post(
        &client,
        &base,
        "/providers/action",
        &action_form(&csrf, chat.id, chat.version, "delete"),
    )
    .await
    .text()
    .await
    .unwrap();
    assert!(rejected_delete.contains("role=\"alert\""));
    assert!(rejected_delete.contains("先停用"));
    let unchanged = store.get(chat.id).await.unwrap();
    assert!(unchanged.enabled && !unchanged.active);
    assert_eq!(unchanged.version, chat.version);

    let editor = client
        .get(format!("{base}/providers/form?id={}", chat.id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(hidden(&editor, "version"), chat.version.to_string());
    assert_eq!(hidden(&editor, "upstream_url"), "https://api.deepseek.com");
    assert!(!editor.contains(PROVIDER_KEY));
    let mut update = provider_form(&csrf, "Chat Renamed", "openai_chat");
    set(&mut update, "upstream_url", "http://127.0.0.1:11434/");
    set(&mut update, "api_key", "");
    set(&mut update, "id", &chat.id.to_string());
    set(&mut update, "version", &chat.version.to_string());
    assert_eq!(
        post(&client, &base, "/providers/save", &update)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let updated = store.get(chat.id).await.unwrap();
    assert_eq!(updated.name, "Chat Renamed");
    assert_eq!(updated.host, "127.0.0.1");
    assert_eq!(updated.port, 11434);
    assert!(!updated.tls);
    assert!(updated.version > chat.version);
    let editor = client
        .get(format!("{base}/providers/form?id={}", chat.id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(hidden(&editor, "upstream_url"), "http://127.0.0.1:11434");
    let conflict = post(&client, &base, "/providers/save", &update)
        .await
        .text()
        .await
        .unwrap();
    assert!(conflict.contains("保存失败"));
    assert!(!conflict.contains(PROVIDER_KEY));
    assert_eq!(store.get(chat.id).await.unwrap().version, updated.version);

    let activate = action_form(&csrf, updated.id, updated.version, "activate");
    assert_eq!(
        post(&client, &base, "/providers/action", &activate)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let active = store.get(chat.id).await.unwrap();
    assert!(active.active && active.enabled);
    assert_eq!(
        store.load_active().await.unwrap()[0].secret,
        PROVIDER_KEY,
        "empty edit key must preserve the credential"
    );
    let list_html = client
        .get(&base)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(list_html.contains("当前使用"));
    assert!(list_html.contains(&format!("id=\"disable-{}\"", active.id)));
    assert!(!list_html.contains(&format!("id=\"delete-{}\"", active.id)));
    assert!(list_html.contains("确认停用「Chat Renamed」？"));
    assert!(
        list_html.contains("取消"),
        "disable confirmation must be localized"
    );
    assert!(!list_html.contains(PROVIDER_KEY));
    let rejected_delete = post(
        &client,
        &base,
        "/providers/action",
        &action_form(&csrf, active.id, active.version, "delete"),
    )
    .await;
    assert!(
        rejected_delete
            .text()
            .await
            .unwrap()
            .contains("role=\"alert\"")
    );
    assert!(store.get(chat.id).await.unwrap().active);
    let disable = action_form(&csrf, active.id, active.version, "disable");
    assert_eq!(
        post(&client, &base, "/providers/action", &disable)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let disabled = store.get(chat.id).await.unwrap();
    assert!(!disabled.active && !disabled.enabled);
    assert!(store.load_active().await.unwrap().is_empty());
    let disabled_html = client
        .get(&base)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(disabled_html.contains(&format!("id=\"delete-{}\"", disabled.id)));
    assert!(!disabled_html.contains(&format!("id=\"disable-{}\"", disabled.id)));
    assert!(disabled_html.contains("确认删除「Chat Renamed」？"));
    assert!(disabled_html.contains("确认删除"));
    assert!(disabled_html.contains("取消"));
    assert_eq!(
        post(
            &client,
            &base,
            "/providers/action",
            &action_form(&csrf, disabled.id, disabled.version, "delete")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(store.list().await.unwrap().len(), 2);
    assert!(store.get(chat.id).await.is_err());
}

fn hidden(html: &str, name: &str) -> String {
    let needle = format!("name=\"{name}\"");
    let remainder = html.split_once(&needle).expect("hidden field exists").1;
    remainder
        .split_once("value=\"")
        .expect("value exists")
        .1
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

fn provider_form(csrf: &str, name: &str, protocol: &str) -> Vec<(String, String)> {
    [
        ("csrf", csrf),
        ("name", name),
        ("protocol", protocol),
        ("upstream_url", "https://api.example.com"),
        ("enabled", "true"),
        ("api_key", PROVIDER_KEY),
        ("anthropic_version", "2023-06-01"),
        ("connect_timeout_ms", "10000"),
        ("read_timeout_ms", "60000"),
        ("write_timeout_ms", "30000"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

fn action_form(csrf: &str, id: i64, version: u64, action: &str) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), csrf.into()),
        ("id".into(), id.to_string()),
        ("version".into(), version.to_string()),
        ("action".into(), action.into()),
    ]
}

fn set(fields: &mut Vec<(String, String)>, key: &str, value: &str) {
    fields.retain(|(name, _)| name != key);
    fields.push((key.to_owned(), value.to_owned()));
}

async fn post(client: &Client, base: &str, path: &str, fields: &[(String, String)]) -> Response {
    client
        .post(format!("{base}{path}"))
        .form(fields)
        .send()
        .await
        .unwrap()
}
