mod support;

use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::{HolidayDate, HolidayKind, ProviderInput, ProviderPaths, ProviderStore};
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

/// Uses a temporary SQLite file and child cwd so the developer's .env,
/// records, and running console cannot affect this HTTP acceptance test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_http_crud_and_request_protection() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-console-database-{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
    exercise_http(&url).await;
    std::fs::remove_dir_all(directory).unwrap();
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
            .get(format!("{base}/providers"))
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

    let response = client
        .get(format!("{base}/providers"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let html = response.text().await.unwrap();
    assert!(html.contains("连接第一个模型服务"));
    assert!(html.contains("/ui/_topcoat/runtime/shards/provider-list"));
    assert_navigation(&html, "/ui/providers", "Providers");
    assert!(html.contains("data-topcoat-usize-bits=\"64\""));
    assert!(html.contains("src=\"/ui/assets/chat-resume.js\""));
    let resume = client
        .get(format!("{base}/assets/chat-resume.js"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(resume.contains("new MutationObserver(resume)"));
    assert!(!html.contains("id=\"routes\""));
    let routes = client.get(format!("{base}/routes")).send().await.unwrap();
    assert_eq!(routes.status(), StatusCode::OK);
    let routes = routes.text().await.unwrap();
    assert_navigation(&routes, "/ui/routes", "Model Routes");
    assert!(!routes.contains("providers-panel"));
    assert!(routes.contains("还没有模型路由"));
    assert_eq!(
        client
            .get(format!("{base}/routes/edit"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let alias = client.get(&base).send().await.unwrap();
    assert_eq!(alias.status(), StatusCode::SEE_OTHER);
    assert_eq!(alias.headers()["location"], "/ui/providers");
    let models = client.get(format!("{base}/models")).send().await.unwrap();
    assert_eq!(models.status(), StatusCode::OK);
    assert_navigation(&models.text().await.unwrap(), "/ui/models", "Models");
    let chat = client.get(format!("{base}/chat")).send().await.unwrap();
    assert_eq!(chat.status(), StatusCode::OK);
    let chat = chat.text().await.unwrap();
    assert_navigation(&chat, "/ui/chat", "Chat");
    assert!(chat.contains("暂无可聊天的模型"));
    let holidays = client.get(format!("{base}/holidays")).send().await.unwrap();
    assert_eq!(holidays.status(), StatusCode::OK);
    let holidays = holidays.text().await.unwrap();
    assert_navigation(&holidays, "/ui/holidays", "节假日");
    assert!(holidays.contains("导入 2026 年官方安排"));
    assert!(holidays.contains("gr-calendar"));
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

    for (path, title) in [
        ("/ui/providers", "Providers"),
        ("/ui/models", "Models"),
        ("/ui/chat", "Chat"),
        ("/ui/routes", "Model Routes"),
        ("/ui/holidays", "节假日"),
    ] {
        let response = client
            .post(format!("{origin}{path}"))
            .header("content-type", "application/json")
            .header("x-topcoat-runtime", "true")
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
    // 停止生成必须注册为实际 HTTP procedure，且仍受 CSRF 校验保护。
    for (token, status) in [
        ("invalid", StatusCode::FORBIDDEN),
        (csrf.as_str(), StatusCode::OK),
    ] {
        let response = client
            .post(format!("{base}/_topcoat/runtime/procedures/stop-chat"))
            .header("content-type", "application/json")
            .json(&serde_json::json!([token, "missing-session"]))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            assert_eq!(response.json::<serde_json::Value>().await.unwrap(), false);
        }
    }
    assert!(editor.contains("type=\"password\""));
    assert!(editor.contains("name=\"upstream_url\""));
    for field in ["host", "port", "tls"] {
        assert!(!editor.contains(&format!("name=\"{field}\"")));
    }
    let bad_host = client
        .get(format!("{base}/providers"))
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
    let mut unsupported_probe = provider_form(&csrf, "Invalid Probe", "openai_chat");
    set(
        &mut unsupported_probe,
        "models_protocol",
        "anthropic_messages",
    );
    assert!(
        post(&client, &base, "/providers/save", &unsupported_probe)
            .await
            .text()
            .await
            .unwrap()
            .contains("模型探测协议必须是已选择的接口协议")
    );
    assert!(store.list().await.unwrap().is_empty());

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
                paths: ProviderPaths::single(Protocol::OpenAiChat),
                host: "legacy.example.com".into(),
                port,
                tls,
                api_key: PROVIDER_KEY.into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::OpenAiChat,
                anthropic_version: None,
                messages_auth: MessagesAuth::ApiKey,
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
            .filter(|provider| provider.paths.anthropic_messages.is_none())
            .all(|provider| provider.anthropic_version.is_none()),
        "non-Anthropic forms must ignore an unrelated version field"
    );
    let chat = providers
        .iter()
        .find(|provider| provider.name == "Chat Test")
        .unwrap();
    let filtered = client
        .get(format!(
            "{base}/providers?q=Chat&protocol=openai_chat&state=enabled"
        ))
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
    assert!(!filtered.contains("设为当前"));
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
    assert!(failure.contains("「Chat Test」删除失败"), "{failure}");
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

    let active = store.get(chat.id).await.unwrap();
    assert!(!active.active && active.enabled);
    let routes = client
        .get(format!("{base}/routes"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(routes.contains("还没有模型路由"));
    assert!(!routes.contains(PROVIDER_KEY));
    assert_eq!(
        store.probe_target(chat.id).await.unwrap().secret,
        PROVIDER_KEY,
        "empty edit key must preserve the credential"
    );
    let list_html = client
        .get(format!("{base}/providers"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!list_html.contains("当前 Chat"));
    assert!(!list_html.contains(&format!("id=\"activate-menu-{}\"", active.id)));
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
    assert!(!store.get(chat.id).await.unwrap().active);
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
            .body("x".repeat(llmproxy_console::BODY_LIMIT + 1))
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
    for action in ["create", "update", "enable", "disable", "delete"] {
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
    let (upstream, received) = support::Mock::http(|request, stream| {
        let body: &[u8] = if request.target == "/custom/models"
            || request.target == "/preview/models"
        {
            br#"{"data":[{"id":"mock-model"},{"id":"mock-model-a"},{"id":"mock-model-b"}]}"#
        } else if request.target == "/v1/chat/completions"
            && request
                .body
                .windows(b"max_tokens".len())
                .any(|part| part == b"max_tokens")
        {
            br#"{"choices":[{"finish_reason":"length"}],"usage":{"prompt_tokens":8,"completion_tokens":1}}"#
        } else {
            br#"{"provider":"ui-configured"}"#
        };
        support::respond(stream, 200, "Content-Type: application/json\r\n", body);
    });
    let mut input = provider_form(&csrf, "Unified Mock", "openai_chat");
    set(
        &mut input,
        "upstream_url",
        &format!("http://{}", upstream.address),
    );
    set(&mut input, "openai_responses", "true");
    set(&mut input, "openai_responses_path", "/custom/responses");
    set(&mut input, "models_path", "/custom/models");
    let preview = post(&client, &base, "/providers/preview", &input).await;
    assert!(preview.text().await.unwrap().contains("mock-model"));
    let preview_request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(preview_request.target, "/custom/models");
    assert_eq!(
        support::values(&preview_request.headers, "authorization"),
        [format!("Bearer {PROVIDER_KEY}")]
    );
    assert!(
        store
            .list()
            .await
            .unwrap()
            .iter()
            .all(|provider| provider.name != "Unified Mock")
    );
    let response = post(&client, &base, "/providers/save", &input).await;
    success_notice(&client, &base, response, "create", "Unified Mock", "已创建").await;
    let mut provider = store
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|provider| provider.name == "Unified Mock")
        .unwrap();
    assert_eq!(
        provider.paths.openai_responses.as_deref(),
        Some("/custom/responses")
    );
    assert_eq!(provider.models_path, "/custom/models");
    assert_eq!(provider.models_probe_status.as_str(), "unprobed");
    let mut preview = input.clone();
    set(&mut preview, "id", &provider.id.to_string());
    set(&mut preview, "version", &provider.version.to_string());
    set(&mut preview, "api_key", "");
    set(&mut preview, "models_path", "/preview/models");
    let response = post(&client, &base, "/providers/preview", &preview).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("mock-model"));
    let request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(request.target, "/preview/models");
    assert_eq!(
        support::values(&request.headers, "authorization"),
        [format!("Bearer {PROVIDER_KEY}")]
    );
    assert_eq!(
        store.get(provider.id).await.unwrap().models_path,
        "/custom/models"
    );
    assert_eq!(
        store
            .get(provider.id)
            .await
            .unwrap()
            .models_probe_status
            .as_str(),
        "unprobed"
    );
    set(&mut preview, "models_path", "/custom/models");
    let response = post(&client, &base, "/providers/preview", &preview).await;
    assert!(response.text().await.unwrap().contains("mock-model"));
    assert_eq!(
        received.recv_timeout(support::DEADLINE).unwrap().target,
        "/custom/models"
    );
    set(&mut preview, "models_probe_status", "success");
    let response = post(&client, &base, "/providers/save", &preview).await;
    success_notice(&client, &base, response, "update", "Unified Mock", "已保存").await;
    provider = store.get(provider.id).await.unwrap();
    assert_eq!(provider.models_probe_status.as_str(), "success");
    let editor = client
        .get(format!("{base}/providers/form?id={}", provider.id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(hidden(&editor, "models_probe_status"), "success");
    let candidates = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/load-model-candidates"
        ))
        .json(&serde_json::json!([provider.id.to_string()]))
        .send()
        .await
        .unwrap();
    assert_eq!(candidates.status(), StatusCode::OK);
    let candidates: serde_json::Value = candidates.json().await.unwrap();
    assert!(
        candidates["ok"]["v"].is_array(),
        "candidates must be typed records"
    );
    assert_eq!(candidates["ok"]["v"][0]["v"]["id"], "mock-model");
    assert_eq!(
        received.recv_timeout(support::DEADLINE).unwrap().target,
        "/custom/models"
    );
    let draft = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/add-draft-model"
        ))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&serde_json::json!(["[]", " manual-model "])).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(draft.status(), StatusCode::OK);
    assert!(draft.text().await.unwrap().contains("manual-model"));
    let model_form = serde_json::json!([csrf, serde_json::json!({
        "id":null, "version":null, "alias":"", "provider_id":provider.id.to_string(), "upstream_model_id":"mock-model",
        "chat":true,"responses":true,"messages":false,"gemini":false,
        "input_price_per_million":"0.15","output_price_per_million":"0.60",
        "thinking_support":"switchable","thinking_mode":"","thinking_effort":"medium","thinking_budget":""
    }).to_string()]);
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/save-model"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&model_form).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value = response.json().await.unwrap();
    assert_eq!(result["ok"], "「Unified Mock/mock-model」已创建");
    let saved_price = store
        .list_models()
        .await
        .unwrap()
        .into_iter()
        .find(|model| model.alias == "Unified Mock/mock-model")
        .unwrap()
        .reference_price
        .unwrap();
    assert_eq!(saved_price.input_per_million, "0.15");
    assert_eq!(saved_price.output_per_million, "0.60");
    let model_id = store
        .list_models()
        .await
        .unwrap()
        .into_iter()
        .find(|model| model.alias == "Unified Mock/mock-model")
        .unwrap()
        .id;
    assert!(store.list_routes().await.unwrap().is_empty());
    let new_route_editor = client
        .get(format!("{base}/routes/edit"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(new_route_editor.contains("route-model-picker"));
    assert!(new_route_editor.contains("id=\"route-protocol\""));
    assert!(new_route_editor.contains("data-protocols=\",openai_chat,openai_responses,\""));
    assert!(new_route_editor.contains("尚未选择候选模型"));
    let selected_table = new_route_editor
        .split("id=\"route-selected-results\"")
        .nth(1)
        .unwrap()
        .split('>')
        .next()
        .unwrap();
    assert!(selected_table.contains("display:none"));
    let route_form = serde_json::json!([
        csrf,
        "",
        "",
        "smart-chat",
        "openai_chat",
        "openai_chat",
        true,
        serde_json::json!([{"model_id": model_id, "enabled": true}]).to_string()
    ]);
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/save-route"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&route_form).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "「smart-chat」已保存"
    );
    let saved_route = store
        .list_routes()
        .await
        .unwrap()
        .into_iter()
        .find(|route| route.name == "smart-chat")
        .unwrap();
    assert_eq!(saved_route.protocol, Protocol::OpenAiChat);
    let route_chat = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(route_chat.contains(&format!("value=\"route:{}\"", saved_route.id)));
    assert!(route_chat.contains("smart-chat · 路由 (Chat)"));
    let route_protocol = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/default-chat-protocol"
        ))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&serde_json::json!([
                csrf,
                format!("route:{}", saved_route.id),
                "gemini"
            ]))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(route_protocol.status(), StatusCode::OK);
    let route_protocol_body = route_protocol.json::<serde_json::Value>().await.unwrap();
    assert_eq!(route_protocol_body, "openai_chat");
    let edit_route = client
        .get(format!("{base}/routes/edit?id={}", saved_route.id))
        .send()
        .await
        .unwrap();
    let edit_status = edit_route.status();
    let edit_html = edit_route.text().await.unwrap();
    assert_eq!(edit_status, StatusCode::OK, "{edit_html}");
    assert!(edit_html.contains("smart-chat"));
    let update_route = serde_json::json!([
        csrf,
        saved_route.id.to_string(),
        saved_route.version.to_string(),
        "smart-chat-v2",
        "openai_chat",
        "openai_chat",
        false,
        serde_json::json!([{"model_id": model_id, "enabled": true}]).to_string()
    ]);
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/save-route"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&update_route).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "「smart-chat-v2」已保存"
    );
    let updated_route = store
        .list_routes()
        .await
        .unwrap()
        .into_iter()
        .find(|route| route.id == saved_route.id)
        .unwrap();
    assert!(!updated_route.enabled);
    let delete_route = serde_json::json!([
        csrf,
        updated_route.id.to_string(),
        updated_route.version.to_string()
    ]);
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/delete-route"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&delete_route).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        store
            .list_routes()
            .await
            .unwrap()
            .iter()
            .all(|route| route.id != saved_route.id)
    );
    let models = client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(models.contains("Unified Mock/mock-model"), "{models}");
    assert!(models.contains("Chat"));
    assert!(models.contains("Responses"));
    assert!(models.contains("可用性探测"));
    assert!(models.contains("探活配置"));
    assert!(models.contains("id=\"model-health-dialog\""));
    assert!(models.contains("未配置"));
    assert!(!models.contains("定时探活 / 健康状态"));
    assert!(!models.contains("开启定时探活"));
    assert!(!models.contains(PROVIDER_KEY));
    let signal = |id: u8, value: serde_json::Value| serde_json::json!({"t":"Signal", "id":format!("{id:032x}"), "v":value});
    let dialog = client
        .post(format!("{base}/_topcoat/runtime/shards/model-health"))
        .header("x-topcoat-identity", "_glY_FmvJupFutmO6b-Ysw")
        .json(&serde_json::json!({"args":[model_id.to_string(), 0.0,
            signal(1, serde_json::json!(0.0)), signal(2, serde_json::json!(true)),
            signal(3, serde_json::json!(false)), signal(4, serde_json::json!(0.0)),
            signal(5, serde_json::json!("")), signal(6, serde_json::json!(""))], "signals":{}}))
        .send()
        .await
        .unwrap();
    let status = dialog.status();
    let dialog = dialog.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{dialog}");
    assert!(dialog.contains("开启定时探活"), "{dialog}");
    assert!(dialog.contains("立即探测"));
    assert!(dialog.contains("最近检查"));
    assert!(dialog.contains("未配置"));
    assert!(dialog.contains("<details"));
    for details in dialog.split("<details").skip(1) {
        let attributes = details.split('>').next().unwrap();
        assert!(
            !attributes.contains(" open"),
            "protocol configuration should start collapsed: {attributes}"
        );
    }
    assert!(models.contains("未探活"));

    for (token, interval, expected_status, expected_key) in [
        ("invalid-csrf", "300", StatusCode::FORBIDDEN, ""),
        (csrf.as_str(), "29", StatusCode::OK, "err"),
        (csrf.as_str(), "300", StatusCode::OK, "ok"),
    ] {
        let response = client
            .post(format!(
                "{base}/_topcoat/runtime/procedures/save-health-check"
            ))
            .json(&serde_json::json!([
                token,
                model_id.to_string(),
                "openai_chat",
                "0",
                false,
                interval,
                "30000",
                "1"
            ]))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        if !expected_key.is_empty() {
            let result = response.json::<serde_json::Value>().await.unwrap();
            assert!(result.get(expected_key).is_some(), "{result}");
        }
    }
    let check = store.model_health_checks(model_id).await.unwrap().remove(0);
    assert!(!check.config.enabled);
    assert_eq!(check.config.interval_seconds, 300);

    assert!(models.contains("参考价格"));
    assert!(models.contains("In: $0.15/M"));
    assert!(models.contains("Out: $0.60/M"));
    let price_rules = serde_json::json!([
        {"t":"Record","v":{"item":"input","time_band":"peak","cache_ttl_seconds":"","prompt_tokens_min":"","prompt_tokens_max":"","unit_price":"0.30"}},
        {"t":"Record","v":{"item":"input","time_band":"off_peak","cache_ttl_seconds":"","prompt_tokens_min":"","prompt_tokens_max":"","unit_price":"0.15"}},
        {"t":"Record","v":{"item":"output","time_band":"peak","cache_ttl_seconds":"","prompt_tokens_min":"","prompt_tokens_max":"","unit_price":"1.20"}},
        {"t":"Record","v":{"item":"output","time_band":"off_peak","cache_ttl_seconds":"","prompt_tokens_min":"","prompt_tokens_max":"","unit_price":"0.60"}}
    ]);
    let windows = serde_json::json!({"t":"Vec","bits":64,"v":[{"t":"Record","v":{"weekday":"1","start":"01:00","end":"04:00"}}]});
    let price_form = serde_json::json!([
        csrf,
        provider.id.to_string(),
        "mock-model",
        "USD",
        "",
        "",
        true,
        "UTC",
        false,
        serde_json::json!({"t":"Vec","bits":64,"v":price_rules}),
        windows
    ]);
    let response = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/save-price-plan"
        ))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&price_form).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value = response.json().await.unwrap();
    assert_eq!(result["ok"], "「Unified Mock/mock-model」参考价格已保存");
    let plan = store
        .get_price_plan(provider.id, "mock-model")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plan.rules.len(), 4);
    assert!(!plan.schedule.as_ref().unwrap().china_holidays_off_peak);
    let preview = |date: &str, holiday_override: bool| {
        serde_json::json!(["UTC", windows, holiday_override, date])
    };
    let response = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/preview-price-band"
        ))
        .json(&preview("2026-09-28T09:30", false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "峰时"
    );
    let response = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/preview-price-band"
        ))
        .json(&preview("2026-09-28T09:30", true))
        .send()
        .await
        .unwrap();
    assert!(
        response.json::<serde_json::Value>().await.unwrap()["ok"]
            .as_str()
            .unwrap()
            .contains("尚未导入")
    );
    store
        .replace_holidays(
            2026,
            vec![HolidayDate {
                date: "2026-10-05".into(),
                name: "国庆节".into(),
                kind: HolidayKind::Holiday,
                source_url: "https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm"
                    .into(),
            }],
        )
        .await
        .unwrap();
    let response = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/preview-price-band"
        ))
        .json(&preview("2026-10-05T09:30", true))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "谷时（中国放假安排）"
    );
    let models = client
        .get(format!("{base}/models"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(models.contains("In: $0.15–$0.30/M"));
    assert!(models.contains("Out: $0.60–$1.20/M"));
    let chat = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(chat.contains("Unified Mock/mock-model"));
    let model_id = store.list_models().await.unwrap()[0].id;
    assert!(chat.contains(&format!("value=\"{model_id}\"")));
    assert!(chat.contains("id=\"chat-protocol\""));
    assert!(!chat.contains("id=\"chat-messages-auth\""));
    assert!(chat.contains("value=\"openai_chat\""));
    assert!(chat.contains("value=\"openai_responses\""));
    assert!(!chat.contains("value=\"anthropic_messages\""));
    let probe_args = vec![
        csrf.clone(),
        model_id.to_string(),
        "openai_chat".to_owned(),
        "1".to_owned(),
    ];
    let probe = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/probe-saved-model"
        ))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&probe_args).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), StatusCode::OK);
    assert!(probe.text().await.unwrap().contains("探测可用"));
    let probe_request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(probe_request.target, "/v1/chat/completions");
    assert_eq!(
        support::values(&probe_request.headers, "authorization"),
        [format!("Bearer {PROVIDER_KEY}")]
    );
    let probe_body: serde_json::Value = serde_json::from_slice(&probe_request.body).unwrap();
    assert_eq!(probe_body["model"], "mock-model");
    assert_eq!(probe_body["max_tokens"], 1);
    assert_eq!(probe_body["thinking"]["type"], "disabled");
    assert_eq!(probe_body["messages"][0]["content"], "你好");
    let mut invalid_probe = probe_args.clone();
    invalid_probe[2] = "anthropic_messages".to_owned();
    let invalid = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/probe-saved-model"
        ))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&invalid_probe).unwrap())
        .send()
        .await
        .unwrap();
    assert!(invalid.text().await.unwrap().contains("模型未配置所选协议"));
    invalid_probe[0] = "invalid-csrf".to_owned();
    let forbidden = client
        .post(format!(
            "{base}/_topcoat/runtime/procedures/probe-saved-model"
        ))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&invalid_probe).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    let routes = client
        .get(format!("{base}/routes"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(routes.contains("还没有模型路由"));
    let mut forwarded = false;
    for _ in 0..60 {
        let response = client
            .post(format!("{origin}/v1/chat/completions"))
            .body(r#"{"model":"Unified Mock/mock-model","messages":[]}"#)
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
        // The one-second snapshot may still have no model mapping.
        assert!(matches!(response.status(), StatusCode::NOT_FOUND));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        forwarded,
        "model save must update the same process's gateway snapshot"
    );
    let request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(
        support::values(&request.headers, "authorization"),
        [format!("Bearer {PROVIDER_KEY}")]
    );
    assert_eq!(request.body, br#"{"model":"mock-model","messages":[]}"#);
    let batch = serde_json::json!({
        "csrf": csrf,
        "provider_id": provider.id.to_string(),
        "models": [
            {"model_id":"mock-model-a","alias":"Unified Mock/model-a","chat":true,"responses":false,"messages":false},
            {"model_id":"mock-model-a","alias":"Unified Mock/duplicate","chat":true,"responses":false,"messages":false}
        ]
    });
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/save-models"))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&serde_json::json!([
                csrf,
                provider.id.to_string(),
                batch["models"].to_string()
            ]))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("模型重复选择"));
    assert_eq!(store.list_models().await.unwrap().len(), 1);
    let mut batch = batch;
    batch["models"][1]["model_id"] = "unlisted-model".into();
    batch["models"][1]["alias"] = "Unified Mock/unlisted-model".into();
    batch["models"][1]["responses"] = true.into();
    batch["models"][1]["chat"] = false.into();
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/save-models"))
        .header("content-type", "application/json")
        .body(
            serde_json::to_vec(&serde_json::json!([
                csrf,
                provider.id.to_string(),
                batch["models"].to_string()
            ]))
            .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "已导入 2 个模型"
    );
    let models = store.list_models().await.unwrap();
    assert_eq!(models.len(), 3);
    assert!(
        models
            .iter()
            .any(|model| model.upstream_model_id == "unlisted-model")
    );
    let list = client
        .get(format!("{base}/providers"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(list.contains("Chat · Resp"));
    assert!(!list.contains("探测模型"));
    let current = store.get(provider.id).await.unwrap();
    let mut edit = provider_form(&csrf, "Unified Mock", "openai_chat");
    set(&mut edit, "id", &current.id.to_string());
    set(&mut edit, "version", &current.version.to_string());
    set(
        &mut edit,
        "upstream_url",
        &format!("http://{}", upstream.address),
    );
    set(&mut edit, "api_key", "");
    set(&mut edit, "openai_responses", "true");
    set(&mut edit, "openai_responses_path", "/custom/responses");
    set(&mut edit, "models_path", "/custom/models");
    set(&mut edit, "anthropic_messages", "true");
    set(&mut edit, "models_protocol", "anthropic_messages");
    let response = post(&client, &base, "/providers/save", &edit).await;
    success_notice(&client, &base, response, "update", "Unified Mock", "已保存").await;
    assert_eq!(
        store
            .get(provider.id)
            .await
            .unwrap()
            .models_probe_status
            .as_str(),
        "unprobed"
    );
    let current = store.get(provider.id).await.unwrap();
    set(&mut edit, "version", &current.version.to_string());
    let response = post(&client, &base, "/providers/preview", &edit).await;
    assert!(response.text().await.unwrap().contains("mock-model"));
    let request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(request.target, "/custom/models");
    assert_eq!(
        support::values(&request.headers, "x-api-key"),
        [PROVIDER_KEY]
    );
    assert_eq!(
        support::values(&request.headers, "anthropic-version"),
        ["2023-06-01"]
    );
    assert_eq!(current.models_probe_status.as_str(), "unprobed");
    set(&mut edit, "models_probe_status", "success");
    let response = post(&client, &base, "/providers/save", &edit).await;
    success_notice(&client, &base, response, "update", "Unified Mock", "已保存").await;
    assert_eq!(
        store
            .get(provider.id)
            .await
            .unwrap()
            .models_probe_status
            .as_str(),
        "success"
    );
    let current = store.get(provider.id).await.unwrap();
    let mut edit = provider_form(&csrf, "Unified Mock", "openai_chat");
    set(&mut edit, "id", &current.id.to_string());
    set(&mut edit, "version", &current.version.to_string());
    set(
        &mut edit,
        "upstream_url",
        &format!("http://{}", upstream.address),
    );
    set(&mut edit, "api_key", "");
    set(&mut edit, "openai_responses", "true");
    set(&mut edit, "anthropic_messages", "true");
    set(&mut edit, "models_protocol", "anthropic_messages");
    set(&mut edit, "models_path", "/missing/models");
    let response = post(&client, &base, "/providers/save", &edit).await;
    success_notice(&client, &base, response, "update", "Unified Mock", "已保存").await;
    let current = store.get(provider.id).await.unwrap();
    set(&mut edit, "version", &current.version.to_string());
    let probe = post(&client, &base, "/providers/preview", &edit).await;
    assert!(
        probe
            .text()
            .await
            .unwrap()
            .contains("上游响应缺少 data 模型列表")
    );
    assert_eq!(
        received.recv_timeout(support::DEADLINE).unwrap().target,
        "/missing/models"
    );
    assert_eq!(current.models_probe_status.as_str(), "unprobed");
    set(&mut edit, "models_probe_status", "failure");
    let response = post(&client, &base, "/providers/save", &edit).await;
    success_notice(&client, &base, response, "update", "Unified Mock", "已保存").await;
    assert_eq!(
        store
            .get(provider.id)
            .await
            .unwrap()
            .models_probe_status
            .as_str(),
        "failure"
    );
    let manual = serde_json::json!([csrf, provider.id.to_string(), serde_json::json!([
        {"model_id":"another-unlisted-model","alias":"","chat":true,"responses":false,"messages":false}
    ]).to_string()]);
    let response = client
        .post(format!("{base}/_topcoat/runtime/procedures/save-models"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&manual).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "已导入 1 个模型"
    );
    assert!(store.list_models().await.unwrap().iter().any(|model| {
        model.alias == "Unified Mock/another-unlisted-model"
            && model.upstream_model_id == "another-unlisted-model"
    }));
    let editor = client
        .get(format!("{base}/providers/form?id={}", provider.id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(hidden(&editor, "models_probe_status"), "failure");
    assert!(editor.contains("API版本"));
    assert!(editor.contains("name=\"messages_auth\""));
    set(
        &mut edit,
        "version",
        &store.get(provider.id).await.unwrap().version.to_string(),
    );
    set(&mut edit, "messages_auth", "bearer");
    let response = post(&client, &base, "/providers/preview", &edit).await;
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("上游响应缺少 data 模型列表")
    );
    let request = received.recv_timeout(support::DEADLINE).unwrap();
    assert_eq!(
        support::values(&request.headers, "authorization"),
        [format!("Bearer {PROVIDER_KEY}")]
    );
    assert!(support::values(&request.headers, "x-api-key").is_empty());
    let response = post(&client, &base, "/providers/save", &edit).await;
    success_notice(&client, &base, response, "update", "Unified Mock", "已保存").await;
    assert_eq!(
        store.get(provider.id).await.unwrap().messages_auth,
        MessagesAuth::Bearer
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
    assert!(events.iter().any(|event| {
        event["fields"]["event_kind"] == "model_probe" && event["fields"]["verdict"] == "available"
    }));
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
            .get(format!("{base}/providers{suffix}"))
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
    client
        .get(format!("{base}/providers"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
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
    assert!(
        html.split("<h1")
            .nth(1)
            .and_then(|heading| heading.split_once("</h1>"))
            .is_some_and(|(heading, _)| heading.contains(title)),
        "missing heading {title}"
    );
    assert!(html.contains(&format!("<strong>{title}</strong>")));
    assert!(!html.contains("href=\"/#routes\""));
    for id in [
        "console-shell",
        "console-sidebar",
        "console-navigation",
        "console-content",
        "console-header",
        "main",
    ] {
        assert_eq!(html.matches(&format!("id=\"{id}\"")).count(), 1);
    }
    let nav = html
        .split_once("<nav")
        .unwrap()
        .1
        .split("</nav>")
        .next()
        .unwrap();
    assert_eq!(nav.matches("aria-current=\"page\"").count(), 1);
    assert_eq!(nav.matches("data-topcoat-link=").count(), 6);
    let chat_link = nav
        .split("<a ")
        .find(|anchor| anchor.contains("href=\"/ui/chat\""))
        .unwrap();
    assert!(chat_link.contains("data-topcoat-link=\"never\""));
    let active = nav
        .split("<a ")
        .find(|anchor| anchor.contains("aria-current=\"page\""))
        .unwrap();
    assert!(active.contains(&format!("href=\"{active_href}\"")));
    assert!(active.contains(&format!(
        "id=\"nav-{}\"",
        active_href.trim_start_matches("/ui/")
    )));
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
    let mut fields: Vec<(String, String)> = [
        ("csrf", csrf),
        ("name", name),
        ("upstream_url", "https://api.example.com"),
        ("enabled", "true"),
        ("api_key", PROVIDER_KEY),
        ("models_path", "/models"),
        ("models_protocol", protocol),
        ("models_probe_status", "unprobed"),
        ("anthropic_version", "2023-06-01"),
        ("messages_auth", "x-api-key"),
        ("connect_timeout_ms", "10000"),
        ("read_timeout_ms", "60000"),
        ("write_timeout_ms", "30000"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    for (name, path) in [
        ("openai_chat", "/v1/chat/completions"),
        ("openai_responses", "/v1/responses"),
        ("anthropic_messages", "/v1/messages"),
    ] {
        fields.push((name.to_owned(), (name == protocol).to_string()));
        fields.push((format!("{name}_path"), path.to_owned()));
    }
    fields
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
    let endpoint = match path {
        "/providers/save" => "save-provider",
        "/providers/preview" => "preview-models",
        "/providers/action" => "provider-action",
        _ => panic!("unknown procedure path: {path}"),
    };
    let value = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map_or("", |(_, value)| value.as_str())
    };
    let args = match path {
        "/providers/save" | "/providers/preview" => {
            let form = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(
                    fields
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.as_str())),
                )
                .finish();
            vec![serde_json::Value::String(form)]
        }
        _ => ["csrf", "id", "version", "action"]
            .into_iter()
            .map(|key| serde_json::Value::String(value(key).to_owned()))
            .collect(),
    };
    client
        .post(format!("{base}/_topcoat/runtime/procedures/{endpoint}"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&args).unwrap())
}
