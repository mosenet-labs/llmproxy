mod support;

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
    std::fs::write(directory.join(".env"), "OTEL_SERVICE_NAME=llmproxy\n").unwrap();
    let rejected = Command::new(env!("CARGO_BIN_EXE_llmproxy"))
        .env_clear()
        .env("LLMPROXY_DATABASE_URL", database_url)
        .env(
            "LLMPROXY_MASTER_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .env("LLMPROXY_LISTEN", format!("127.0.0.1:{port}"))
        .current_dir(&directory)
        .output()
        .expect("start console with a wrong master key");
    assert!(
        !rejected.status.success(),
        "wrong master key must fail before listening"
    );
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains(database_url));
    let log_path = directory.join("console.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_llmproxy"))
        .env_clear()
        .env("LLMPROXY_DATABASE_URL", database_url)
        .env("LLMPROXY_MASTER_KEY", MASTER_KEY)
        .env("LLMPROXY_LISTEN", format!("127.0.0.1:{port}"))
        .current_dir(&directory)
        .stdout(log)
        .stderr(Stdio::null())
        .spawn()
        .expect("start console process");
    let mut process = ConsoleProcess { child, directory };
    let origin = format!("http://127.0.0.1:{port}");
    let base = format!("{origin}/ui");
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
    assert_navigation(&html, "/ui", "Provider 管理");
    assert!(!html.contains("id=\"routes\""));
    let routes = client.get(format!("{base}/routes")).send().await.unwrap();
    assert_eq!(routes.status(), StatusCode::OK);
    let routes = routes.text().await.unwrap();
    assert_navigation(&routes, "/ui/routes", "路由概览");
    assert!(!routes.contains("providers-panel"));
    assert_eq!(routes.matches("尚未分配").count(), 3);
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        assert!(routes.contains(path));
    }
    let alias = client
        .get(format!("{base}/providers"))
        .send()
        .await
        .unwrap();
    assert_eq!(alias.status(), StatusCode::SEE_OTHER);
    assert_eq!(alias.headers()["location"], "/ui");
    for asset in ["components.css", "console.css", "runtime.js"] {
        let response = client
            .get(format!("{base}/assets/{asset}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{asset}");
        let body = response.text().await.unwrap();
        assert!(body.len() > 500, "{asset}");
        if asset == "console.css" {
            // Serve generated CSS, never the uncompiled Tailwind entry point.
            assert!(body.contains("tailwindcss"));
            assert!(body.contains("grid-template-columns:216px minmax(0,1fr)"));
            assert!(body.contains("--color-primary:"));
            assert!(!body.contains("@source"));
            assert!(!body.contains("@apply"));
        }
        if asset == "runtime.js" {
            for endpoint in ["procedures", "shards", "pages"] {
                assert!(body.contains(&format!("/ui/_topcoat/runtime/{endpoint}")));
            }
        }
    }
    let font_href = html
        .split("href=\"")
        .skip(1)
        .map(|value| value.split('"').next().unwrap())
        .find(|value| value.starts_with("/ui/_topcoat/fonts/"))
        .expect("prefixed font stylesheet");
    let font_css = client
        .get(format!("{origin}{font_href}"))
        .send()
        .await
        .unwrap();
    assert_eq!(font_css.status(), StatusCode::OK);
    assert!(
        font_css
            .text()
            .await
            .unwrap()
            .contains("font-family: \"JetBrains Mono\"")
    );

    for (path, title) in [("/ui", "Provider 管理"), ("/ui/routes", "路由概览")] {
        let response = client
            .post(format!("{base}/_topcoat/runtime/pages{path}"))
            .header("content-type", "application/json")
            .body(r#"{"signals":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "native page re-run {path}"
        );
        assert_navigation(&response.text().await.unwrap(), path, title);
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
    for (header, value) in [
        ("host", "rebind.invalid"),
        ("origin", "https://foreign.invalid"),
    ] {
        let response = procedure_request(
            &client,
            &base,
            "/providers/save",
            &provider_form(&csrf, "Blocked", "openai_chat"),
        )
        .await
        .header(header, value)
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
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
            "Messages 测试 & Backup",
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
        success_notice(&client, &base, response, "created", name, "已创建").await;
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
            .filter(|provider| provider.protocol != Protocol::AnthropicMessages)
            .all(|provider| provider.anthropic_version.is_none()),
        "non-Anthropic forms must ignore an unrelated version field"
    );
    let chat = providers
        .iter()
        .find(|provider| provider.name == "Chat Test")
        .unwrap();
    let filtered = client
        .get(format!("{base}?q=Chat&protocol=openai_chat&state=enabled"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(filtered.contains("Chat Test"));
    assert!(!filtered.contains("Responses Test"));
    assert!(!filtered.contains("Messages 测试"));
    assert!(!filtered.contains(PROVIDER_KEY));
    assert!(filtered.contains("设为当前服务"));
    assert!(filtered.contains(&format!("id=\"disable-{}\"", chat.id)));
    assert!(!filtered.contains(&format!("id=\"delete-{}\"", chat.id)));
    assert!(filtered.contains("确认停用「Chat Test」？"));
    assert!(filtered.contains("确认停用"));
    confirmation_without_description(
        &filtered,
        &format!("disable-{}", chat.id),
        "确认停用「Chat Test」？",
    );

    // Hiding deletion in the UI must also be enforced for a direct POST, even
    // when the enabled provider has not been selected for the current route.
    let mut delete_fields = action_form(&csrf, chat.id, chat.version, "delete");
    set(&mut delete_fields, "provider_name", "伪造名称");
    let rejected_delete = post(&client, &base, "/providers/action", &delete_fields)
        .await
        .text()
        .await
        .unwrap();
    let failure = &rejected_delete;
    assert!(failure.contains("「Chat Test」删除失败"));
    assert!(failure.contains("先停用"));
    assert!(!failure.contains("伪造名称"));
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
    let response = post(&client, &base, "/providers/save", &update).await;
    success_notice(
        &client,
        &base,
        response,
        "updated",
        "Chat Renamed",
        "已保存",
    )
    .await;
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
    let response = post(&client, &base, "/providers/action", &activate).await;
    success_notice(
        &client,
        &base,
        response,
        "activate",
        "Chat Renamed",
        "已设为当前服务",
    )
    .await;
    let active = store.get(chat.id).await.unwrap();
    assert!(active.active && active.enabled);
    let routes = client
        .get(format!("{base}/routes"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(routes.contains("Chat Renamed"));
    assert!(routes.contains("当前 Provider"));
    assert!(!routes.contains(PROVIDER_KEY));
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
    confirmation_without_description(
        &list_html,
        &format!("disable-{}", active.id),
        "确认停用「Chat Renamed」？",
    );
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
    .await
    .text()
    .await
    .unwrap();
    let failure = &rejected_delete;
    assert!(failure.contains("「Chat Renamed」删除失败"));
    assert!(failure.contains("先停用"));
    assert!(store.get(chat.id).await.unwrap().active);
    let disable = action_form(&csrf, active.id, active.version, "disable");
    let response = post(&client, &base, "/providers/action", &disable).await;
    let disabled_html = success_notice(
        &client,
        &base,
        response,
        "disable",
        "Chat Renamed",
        "已停用",
    )
    .await;
    let disabled = store.get(chat.id).await.unwrap();
    assert!(!disabled.active && !disabled.enabled);
    assert!(store.load_active().await.unwrap().is_empty());
    assert!(disabled_html.contains(&format!("id=\"delete-{}\"", disabled.id)));
    assert!(!disabled_html.contains(&format!("id=\"disable-{}\"", disabled.id)));
    assert!(disabled_html.contains("确认删除「Chat Renamed」？"));
    assert!(disabled_html.contains("确认删除"));
    assert!(disabled_html.contains("取消"));
    confirmation_without_description(
        &disabled_html,
        &format!("delete-{}", disabled.id),
        "确认删除「Chat Renamed」？",
    );

    let enable = action_form(&csrf, disabled.id, disabled.version, "enable");
    let response = post(&client, &base, "/providers/action", &enable).await;
    success_notice(&client, &base, response, "enable", "Chat Renamed", "已启用").await;
    let enabled = store.get(chat.id).await.unwrap();
    assert!(enabled.enabled && !enabled.active);
    let disable = action_form(&csrf, enabled.id, enabled.version, "disable");
    let response = post(&client, &base, "/providers/action", &disable).await;
    success_notice(
        &client,
        &base,
        response,
        "disable",
        "Chat Renamed",
        "已停用",
    )
    .await;
    let disabled = store.get(chat.id).await.unwrap();
    let response = post(
        &client,
        &base,
        "/providers/action",
        &action_form(&csrf, disabled.id, disabled.version, "delete"),
    )
    .await;
    success_notice(&client, &base, response, "delete", "Chat Renamed", "已删除").await;
    assert_eq!(store.list().await.unwrap().len(), 2);
    assert!(store.get(chat.id).await.is_err());

    let unknown_notice = client
        .get(format!(
            "{base}?notice=unknown&provider_name=UntrustedNotice"
        ))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!unknown_notice.contains("UntrustedNotice"));

    assert_eq!(
        client
            .get(format!("{base}/unknown-private-path?key=secret-query"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .put(format!("{base}/providers/save"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client
            .post(format!("{base}/providers/save"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body("x".repeat(40 * 1024))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    let logs = std::fs::read_to_string(&log_path).unwrap();
    for secret in [
        PROVIDER_KEY,
        MASTER_KEY,
        database_url,
        csrf.as_str(),
        "secret-query",
        "unknown-private-path",
    ] {
        assert!(
            !logs.contains(secret),
            "sensitive value leaked to console logs"
        );
    }
    let events: Vec<serde_json::Value> = logs
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON log line"))
        .collect();
    assert!(
        events
            .iter()
            .any(|event| event["fields"]["service_name"] == "llmproxy")
    );
    let requests: Vec<_> = events
        .iter()
        .filter(|event| event["fields"]["event_kind"] == "request")
        .collect();
    for status in [200, 303, 403, 404, 405, 413] {
        assert!(
            requests
                .iter()
                .any(|event| event["fields"]["status"] == status),
            "missing request status {status}"
        );
    }
    assert!(
        requests
            .iter()
            .all(|event| event["fields"]["component"] == "console")
    );
    let ids: std::collections::HashSet<_> = requests
        .iter()
        .map(|event| event["fields"]["request_id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids.len(), requests.len(), "one event per request");
    let operations: Vec<_> = events
        .iter()
        .filter(|event| event["fields"]["event_kind"] == "provider_operation")
        .collect();
    for action in [
        "create", "update", "activate", "enable", "disable", "delete",
    ] {
        assert!(
            operations
                .iter()
                .any(|event| event["fields"]["action"] == action
                    && event["fields"]["outcome"] == "success"
                    && event["fields"]["provider_name"].as_str().is_some()),
            "missing named operation {action}"
        );
    }
    for kind in ["validation", "conflict"] {
        assert!(
            operations
                .iter()
                .any(|event| event["fields"]["error_kind"] == kind
                    && event["fields"]["outcome"] == "failure"),
            "missing failure {kind}"
        );
    }

    // The same listener owns UI routes and LLM requests. No real provider is called.
    for path in [
        "/uix",
        "/ui-other",
        "/assets/runtime.js",
        "/_topcoat/runtime/procedures/nope",
        "/providers/save",
    ] {
        assert_eq!(
            client
                .get(format!("{origin}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let (upstream, received) = support::Mock::http(|_, stream| {
        support::respond(
            stream,
            200,
            "Content-Type: application/json\r\n",
            br#"{"provider":"ui-configured"}"#,
        );
    });
    let mut input = provider_form(&csrf, "Unified Mock", "openai_chat");
    set(
        &mut input,
        "upstream_url",
        &format!("http://{}", upstream.address),
    );
    let response = post(&client, &base, "/providers/save", &input).await;
    success_notice(&client, &base, response, "create", "Unified Mock", "已创建").await;
    let provider = store
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|provider| provider.name == "Unified Mock")
        .unwrap();
    let response = post(
        &client,
        &base,
        "/providers/action",
        &action_form(&csrf, provider.id, provider.version, "activate"),
    )
    .await;
    success_notice(
        &client,
        &base,
        response,
        "activate",
        "Unified Mock",
        "已设为当前服务",
    )
    .await;
    let mut forwarded = false;
    for _ in 0..60 {
        let response = client
            .post(format!("{origin}/v1/chat/completions"))
            .body(r#"{"model":"mock","messages":[]}"#)
            .send()
            .await
            .unwrap();
        if response.status() == StatusCode::OK {
            assert_eq!(
                response.text().await.unwrap(),
                r#"{"provider":"ui-configured"}"#
            );
            forwarded = true;
            break;
        }
        // The preceding CRUD cases may still be in the one-second snapshot.
        // An old placeholder upstream returns 502; an unbound snapshot returns 503.
        assert!(matches!(
            response.status(),
            StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        forwarded,
        "UI activation must update the same process's gateway snapshot"
    );
    let request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(
        support::values(&request.headers, "authorization"),
        [format!("Bearer {PROVIDER_KEY}")]
    );
    let logs = std::fs::read_to_string(&log_path).unwrap();
    let events: Vec<serde_json::Value> = logs
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["fields"]["message"] == "telemetry initialized")
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| event["fields"]["component"] == "gateway"
                && event["fields"]["route"] == "/v1/chat/completions"
                && event["fields"]["status"] == 200)
    );
    assert!(!logs.contains(PROVIDER_KEY));
}

async fn success_notice(
    client: &Client,
    base: &str,
    response: Response,
    notice: &str,
    provider_name: &str,
    result: &str,
) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("location"));
    assert!(!response.headers().contains_key("set-cookie"));
    let response = response.text().await.unwrap();
    let outcome: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(outcome["t"], "Result");
    assert_eq!(outcome["ok"], format!("「{provider_name}」{result}"));
    assert!(!response.contains(PROVIDER_KEY));
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("notice", notice)
        .append_pair("provider_name", provider_name)
        .finish();
    // Both a plain refresh and an old success URL must start without feedback.
    for suffix in [String::new(), format!("?{query}"), format!("?{query}")] {
        let html = client
            .get(format!("{base}{suffix}"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!html.contains(&format!(
            "「{}」{result}",
            provider_name.replace('&', "&amp;")
        )));
        assert!(!html.contains("data-state=\"open\""));
    }
    client.get(base).send().await.unwrap().text().await.unwrap()
}

fn confirmation_without_description(html: &str, id: &str, title: &str) {
    let confirmation = element(html, "aside", &format!("id=\"{id}\""));
    assert!(confirmation.contains(title));
    assert!(!confirmation.contains("aria-describedby"));
    assert!(!confirmation.contains(&format!("id=\"{id}-description\"")));
}

fn element<'a>(html: &'a str, tag: &str, marker: &str) -> &'a str {
    html.split(&format!("<{tag}"))
        .skip(1)
        .find_map(|section| {
            let element = section.split_once(&format!("</{tag}>"))?.0;
            element.contains(marker).then_some(element)
        })
        .expect("expected HTML element")
}

fn assert_navigation(html: &str, active_href: &str, title: &str) {
    assert!(html.contains(&format!("<title>{title} · LLMProxy</title>")));
    assert!(html.contains(&format!("<h1>{title}</h1>")));
    assert!(html.contains(&format!("<strong>{title}</strong>")));
    assert!(!html.contains("href=\"/#routes\""));
    let nav = html
        .split_once("<nav")
        .unwrap()
        .1
        .split("</nav>")
        .next()
        .unwrap();
    assert_eq!(nav.matches("aria-current=\"page\"").count(), 1);
    let active = nav
        .split("<a ")
        .find(|anchor| anchor.contains("aria-current=\"page\""))
        .unwrap();
    assert!(active.contains(&format!("href=\"{active_href}\"")));
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

// Exercise the same native procedures wired into the rendered forms. Procedure
// IDs are generated by Topcoat, so discover them from the page instead of
// coupling the test to an ID that changes on each compilation.
async fn post(client: &Client, base: &str, path: &str, fields: &[(String, String)]) -> Response {
    procedure_request(client, base, path, fields)
        .await
        .send()
        .await
        .unwrap()
}

async fn procedure_request(
    client: &Client,
    base: &str,
    path: &str,
    fields: &[(String, String)],
) -> reqwest::RequestBuilder {
    let html = client.get(base).send().await.unwrap().text().await.unwrap();
    let form = element(&html, "form", &format!("action=\"/ui{path}\""));
    let decoded = form.replace("&quot;", "\"");
    let id = decoded
        .split_once("\"t\":\"Procedure\",\"id\":\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    let keys: &[&str] = if path == "/providers/save" {
        &[
            "csrf",
            "id",
            "version",
            "name",
            "protocol",
            "upstream_url",
            "enabled",
            "api_key",
            "anthropic_version",
            "connect_timeout_ms",
            "read_timeout_ms",
            "write_timeout_ms",
        ]
    } else {
        &["csrf", "id", "version", "action"]
    };
    let args: Vec<serde_json::Value> = keys
        .iter()
        .map(|key| {
            let value = fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .unwrap_or("");
            if *key == "enabled" {
                serde_json::Value::Bool(value == "true")
            } else {
                serde_json::Value::String(value.to_owned())
            }
        })
        .collect();
    client
        .post(format!("{base}/_topcoat/runtime/procedures/{id}"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&args).unwrap())
}
