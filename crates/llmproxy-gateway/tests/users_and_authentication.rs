#[path = "support/mail.rs"]
mod mail;
#[allow(dead_code)]
mod nonstream;
mod support;

use llmproxy_store::auth::{EmailPurpose, UserRole};
use nonstream::{Database, MASTER_KEY};
use reqwest::{Client, Response, StatusCode, redirect::Policy};
use support::{DEADLINE, Gateway, Mock};

fn csrf(html: &str) -> String {
    html.split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}

async fn call(client: &Client, base: &str, action: &str, fields: &[(&str, &str)]) -> Response {
    let payload = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish();
    client
        .post(format!("{base}/ui/_topcoat/runtime/procedures/{action}"))
        .header("content-type", "application/json")
        .header("origin", base)
        .body(serde_json::to_vec(&[payload]).unwrap())
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_admin_is_initialized_on_the_login_page_once_without_smtp() {
    let database = Database::new().await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let base = format!("http://{}", gateway.address);
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .build()
        .unwrap();
    let html = client
        .get(format!("{base}/ui/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("id=\"auth-bootstrap-form\""));
    assert!(html.contains("创建管理员并登录"));
    assert!(html.contains("id=\"bootstrap-password-fields\""));
    assert!(html.contains("id=\"bootstrap-confirmation-error\""));
    assert!(html.contains("两次输入的密码不一致"));
    assert!(html.contains("密码须为 15–128 个字符。"));
    assert!(!html.contains("cargo run"));
    let public_csrf = csrf(&html);
    let rejected = call(
        &client,
        &base,
        "auth-bootstrap-admin",
        &[
            ("csrf", "wrong"),
            ("email", "admin@example.test"),
            ("password", "isolated-test-password"),
            ("confirmation", "isolated-test-password"),
        ],
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    for (email, password, confirmation, message) in [
        (
            "invalid",
            "isolated-test-password",
            "isolated-test-password",
            "请输入有效的邮箱地址",
        ),
        ("admin@example.test", "short", "short", "密码须为"),
        (
            "admin@example.test",
            "isolated-test-password",
            "different",
            "两次输入的密码不一致",
        ),
    ] {
        let rejected = call(
            &client,
            &base,
            "auth-bootstrap-admin",
            &[
                ("csrf", &public_csrf),
                ("email", email),
                ("password", password),
                ("confirmation", confirmation),
            ],
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::OK);
        assert!(!rejected.headers().contains_key("set-cookie"));
        assert!(rejected.text().await.unwrap().contains(message));
    }
    assert!(database.store.list_users().await.unwrap().is_empty());
    let first_fields = [
        ("csrf", public_csrf.as_str()),
        ("email", "first@example.test"),
        ("password", "isolated-test-password"),
        ("confirmation", "isolated-test-password"),
    ];
    let second_fields = [
        ("csrf", public_csrf.as_str()),
        ("email", "second@example.test"),
        ("password", "isolated-test-password"),
        ("confirmation", "isolated-test-password"),
    ];
    let (first, second) = tokio::join!(
        call(&client, &base, "auth-bootstrap-admin", &first_fields),
        call(&client, &base, "auth-bootstrap-admin", &second_fields)
    );
    let (created, rejected) = if first.headers().contains_key("set-cookie") {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(created.status(), StatusCode::OK);
    let cookie = created.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(created.text().await.unwrap().contains("/ui/chat"));
    assert!(!rejected.headers().contains_key("set-cookie"));
    assert!(rejected.text().await.unwrap().contains("管理员已初始化"));
    let users = database.store.list_users().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].role, UserRole::Admin);
    assert!(users[0].email_verified);
    assert!(users[0].enabled);
    assert_eq!(
        client
            .get(format!("{base}/ui/users"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let html = client
        .get(format!("{base}/ui/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!html.contains("id=\"auth-bootstrap-form\""));
    assert!(!html.contains("id=\"auth-tab-bootstrap\""));
    let repeated = call(&client, &base, "auth-bootstrap-admin", &first_fields).await;
    assert!(!repeated.headers().contains_key("set-cookie"));
    assert!(repeated.text().await.unwrap().contains("管理员已初始化"));
    assert_eq!(database.store.list_users().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_requires_sessions_and_opens_personal_resources_for_members() {
    let database = Database::new().await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let base = format!("http://{}", gateway.address);
    let anonymous = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .build()
        .unwrap();
    let response = anonymous
        .get(format!("{base}/ui/providers"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/ui/login");
    let html = anonymous
        .get(format!("{base}/ui/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("id=\"auth-bootstrap-form\""));
    assert!(html.contains("管理员尚未配置邮件服务"));
    let public_csrf = csrf(&html);
    for path in [
        "/ui/_topcoat/runtime/procedures/save-group",
        "/ui/_topcoat/runtime/shards/model-workspace",
        "/ui/_topcoat/runtime/procedures/save-mail-settings",
        "/ui/_topcoat/runtime/procedures/test-mail-settings",
    ] {
        let response = anonymous
            .post(format!("{base}{path}"))
            .header("content-type", "application/json")
            .body("[]")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let rejected = call(
        &anonymous,
        &base,
        "auth-login",
        &[
            ("csrf", "wrong"),
            ("email", "admin@example.test"),
            ("password", "isolated-test-password"),
        ],
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let admin = gateway
        .console_client()
        .redirect(Policy::none())
        .build()
        .unwrap();
    assert_eq!(
        admin
            .get(format!("{base}/v1/models"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let virtual_cookie = anonymous
        .post(format!("{base}/ui/_topcoat/runtime/procedures/save-group"))
        .header("cookie", format!("llmproxy_session={}", gateway.api_key))
        .body("[]")
        .send()
        .await
        .unwrap();
    assert_eq!(virtual_cookie.status(), StatusCode::UNAUTHORIZED);
    let html = admin
        .get(format!("{base}/ui/users"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let admin_csrf = csrf(&html);
    assert!(html.contains("用户列表"));
    assert!(html.contains("users-page-size"));
    let login = call(
        &anonymous,
        &base,
        "auth-login",
        &[
            ("csrf", &public_csrf),
            ("email", "ADMIN@example.test"),
            ("password", "isolated-test-password"),
        ],
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Path=/ui"));
    assert!(!login.text().await.unwrap().contains("lp-us-"));
    let code = database
        .store
        .issue_email_code("member@example.test", EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    database
        .store
        .register_user(&code.email, &code.code, "isolated-test-password", "Member")
        .await
        .unwrap();
    let member = database
        .store
        .login(&code.email, "isolated-test-password")
        .await
        .unwrap()
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "cookie",
        format!("llmproxy_session={}", member.secret)
            .parse()
            .unwrap(),
    );
    let member_client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .default_headers(headers)
        .build()
        .unwrap();
    let html = member_client
        .get(format!("{base}/ui/account"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("个人账户"));
    assert!(html.contains("id=\"account-confirmation-error\""));
    assert!(!html.contains("id=\"nav-users\""));
    for menu in [
        "chat",
        "providers",
        "models",
        "routes",
        "subscriptions",
        "keys",
        "account",
    ] {
        assert!(
            html.contains(&format!("id=\"nav-{menu}\"")),
            "missing menu {menu}"
        );
    }
    assert!(!html.contains("当前仅开放个人账户功能"));
    assert!(!html.contains("id=\"nav-settings\""));
    for path in [
        "providers",
        "models",
        "groups",
        "chat",
        "routes",
        "subscriptions",
        "keys",
    ] {
        let response = member_client
            .get(format!("{base}/ui/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "member page {path}");
    }
    for path in ["users", "settings", "holidays"] {
        assert_eq!(
            member_client
                .get(format!("{base}/ui/{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let saved = call(
        &member_client,
        &base,
        "save-group",
        &[
            ("csrf", &member.session.csrf),
            ("name", "personal-test"),
            ("group_id", "0"),
        ],
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert!(saved.text().await.unwrap().contains("组已创建"));
    assert!(
        database
            .store
            .for_user(member.session.user.id)
            .await
            .unwrap()
            .list_groups()
            .await
            .unwrap()
            .iter()
            .any(|group| group.name == "personal-test")
    );
    for action in ["save-mail-settings", "test-mail-settings"] {
        let denied = call(
            &member_client,
            &base,
            action,
            &[("csrf", &member.session.csrf)],
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let cross_session = call(
        &admin,
        &base,
        "auth-save-profile",
        &[("csrf", &member.session.csrf), ("display_name", "Cross")],
    )
    .await;
    assert_eq!(cross_session.status(), StatusCode::FORBIDDEN);
    let saved = call(
        &member_client,
        &base,
        "auth-save-profile",
        &[
            ("csrf", &member.session.csrf),
            ("display_name", "Updated Member"),
        ],
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert!(saved.text().await.unwrap().contains("个人信息已保存"));
    let user = database
        .store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.id == member.session.user.id)
        .unwrap();
    let response = call(
        &admin,
        &base,
        "save-user",
        &[
            ("csrf", &admin_csrf),
            ("id", &user.id.to_string()),
            ("version", &user.version.to_string()),
            ("display_name", "Disabled Member"),
            ("role", "user"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("用户信息已保存"));
    assert_eq!(
        member_client
            .get(format!("{base}/ui/account"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
    let admin_user = database
        .store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.role == UserRole::Admin)
        .unwrap();
    let rejected = call(
        &admin,
        &base,
        "save-user",
        &[
            ("csrf", &admin_csrf),
            ("id", &admin_user.id.to_string()),
            ("version", &admin_user.version.to_string()),
            ("display_name", "Admin"),
            ("role", "user"),
            ("enabled", "true"),
        ],
    )
    .await;
    assert!(
        rejected
            .text()
            .await
            .unwrap()
            .contains("最后一个有效管理员")
    );
    let logout = call(&admin, &base, "auth-logout", &[("csrf", &admin_csrf)]).await;
    assert_eq!(logout.status(), StatusCode::OK);
    assert!(
        logout.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        admin
            .get(format!("{base}/ui/users"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smtp_registration_and_password_recovery_use_real_public_procedures() {
    let database = Database::new().await;
    let (smtp, inbox) = mail::smtp();
    let smtp_port = smtp.address.port().to_string();
    let gateway = Gateway::database_with_environment(
        &database.url,
        MASTER_KEY,
        &[("LLMPROXY_SMTP_HOST", "ignored-legacy-environment.invalid")],
    );
    let base = format!("http://{}", gateway.address);
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .build()
        .unwrap();
    let initial = client
        .get(format!("{base}/ui/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(initial.contains("管理员尚未配置邮件服务"));
    let admin = gateway
        .console_client()
        .redirect(Policy::none())
        .build()
        .unwrap();
    let settings = admin
        .get(format!("{base}/ui/settings"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(settings.contains("邮件服务"));
    assert!(settings.contains("id=\"nav-settings\""));
    let settings_csrf = csrf(&settings);
    let mut fields = vec![
        ("csrf", "wrong"),
        ("enabled", "true"),
        ("host", "127.0.0.1"),
        ("port", smtp_port.as_str()),
        ("tls", "none"),
        ("from", "noreply@example.test"),
    ];
    let rejected = call(&admin, &base, "save-mail-settings", &fields).await;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    fields[0].1 = &settings_csrf;
    let saved = call(&admin, &base, "save-mail-settings", &fields).await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert!(saved.text().await.unwrap().contains("立即生效"));
    assert!(database.store.mail_enabled().await.unwrap());
    let sent = call(
        &admin,
        &base,
        "test-mail-settings",
        &[
            ("csrf", &settings_csrf),
            ("recipient", "admin@example.test"),
        ],
    )
    .await;
    assert!(sent.text().await.unwrap().contains("测试邮件已发送"));
    let test_message = inbox.recv_timeout(DEADLINE).unwrap();
    assert!(test_message.contains("admin@example.test"));
    assert!(test_message.contains("noreply@example.test"));
    assert!(
        test_message
            .to_lowercase()
            .contains("content-type: text/plain; charset=utf-8")
    );
    let html = client
        .get(format!("{base}/ui/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = csrf(&html);
    assert!(html.contains("id=\"register-confirmation-error\""));
    assert!(html.contains("id=\"reset-confirmation-error\""));
    assert!(!html.contains("name=\"display_name\""));
    let email = "new@example.test";
    let sent = call(
        &client,
        &base,
        "auth-send-code",
        &[("csrf", &csrf), ("email", email), ("purpose", "register")],
    )
    .await;
    assert_eq!(sent.status(), StatusCode::OK);
    assert!(sent.text().await.unwrap().contains("验证码已发送"));
    let message = inbox.recv_timeout(DEADLINE).unwrap();
    assert!(message.contains(email));
    assert!(
        message
            .to_lowercase()
            .contains("content-type: text/plain; charset=utf-8")
    );
    let code = mail::code(&message);
    let result = call(
        &client,
        &base,
        "auth-register",
        &[
            ("csrf", &csrf),
            ("email", email),
            ("code", &code),
            ("password", "isolated-test-password"),
            ("confirmation", "isolated-test-password"),
        ],
    )
    .await;
    assert!(result.text().await.unwrap().contains("注册成功"));
    let created = database
        .store
        .login(email, "isolated-test-password")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created.session.user.display_name, email);
    let logged = call(
        &client,
        &base,
        "auth-login",
        &[
            ("csrf", &csrf),
            ("email", email),
            ("password", "isolated-test-password"),
        ],
    )
    .await;
    let cookie = logged.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(logged.text().await.unwrap().contains("/ui/chat"));
    let sent = call(
        &client,
        &base,
        "auth-send-code",
        &[
            ("csrf", &csrf),
            ("email", email),
            ("purpose", "reset_password"),
        ],
    )
    .await;
    assert!(sent.text().await.unwrap().contains("验证码已发送"));
    let code = mail::code(&inbox.recv_timeout(DEADLINE).unwrap());
    let reset = call(
        &client,
        &base,
        "auth-reset-password",
        &[
            ("csrf", &csrf),
            ("email", email),
            ("code", &code),
            ("password", "new-isolated-password"),
            ("confirmation", "new-isolated-password"),
        ],
    )
    .await;
    assert!(reset.text().await.unwrap().contains("密码已重置"));
    let expired = client
        .get(format!("{base}/ui/account"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), StatusCode::SEE_OTHER);
    let logged = call(
        &client,
        &base,
        "auth-login",
        &[
            ("csrf", &csrf),
            ("email", email),
            ("password", "new-isolated-password"),
        ],
    )
    .await;
    assert!(logged.headers().contains_key("set-cookie"));
    assert!(logged.text().await.unwrap().contains("/ui/chat"));
    let version = database
        .store
        .mail_settings()
        .await
        .unwrap()
        .unwrap()
        .version
        .to_string();
    fields.retain(|(name, _)| *name != "enabled");
    fields.push(("version", &version));
    let disabled = call(&admin, &base, "save-mail-settings", &fields).await;
    assert!(disabled.text().await.unwrap().contains("立即生效"));
    let rejected = call(
        &client,
        &base,
        "auth-send-code",
        &[
            ("csrf", &csrf),
            ("email", "disabled@example.test"),
            ("purpose", "register"),
        ],
    )
    .await;
    assert!(rejected.text().await.unwrap().contains("未配置邮件服务"));
    let rejected = call(
        &admin,
        &base,
        "test-mail-settings",
        &[
            ("csrf", &settings_csrf),
            ("recipient", "admin@example.test"),
        ],
    )
    .await;
    assert!(rejected.text().await.unwrap().contains("请先保存并启用"));
    drop(gateway);
    let restarted = Gateway::database(&database.url, MASTER_KEY);
    let html = client
        .get(format!("http://{}/ui/login", restarted.address))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("管理员尚未配置邮件服务"));
    assert!(!html.contains("id=\"auth-register-form\""));
}
