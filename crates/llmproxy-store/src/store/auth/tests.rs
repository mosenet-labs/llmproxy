use super::*;
use base64::engine::general_purpose::STANDARD;

const PASSWORD: &str = "isolated-test-password";
const NEW_PASSWORD: &str = "another-isolated-password";

fn mail_input() -> crate::settings::MailSettingsInput {
    crate::settings::MailSettingsInput {
        enabled: true,
        host: "smtp.example.test".into(),
        port: 587,
        tls: "starttls".into(),
        from: "noreply@example.test".into(),
        username: "mail-user".into(),
        password: "isolated-smtp-secret".into(),
        version: None,
    }
}

#[tokio::test]
async fn mail_settings_encrypt_preserve_replace_and_persist_credentials() {
    use crate::model::MailSettingsRow;
    let (store, directory) = database().await;
    let admin = store
        .bootstrap_admin("admin@example.test", PASSWORD)
        .await
        .unwrap();
    assert!(!store.mail_enabled().await.unwrap());
    let saved = store
        .save_mail_settings(admin.id, mail_input())
        .await
        .unwrap();
    assert!(saved.password_configured);
    let mut connection = store.connection().await.unwrap();
    let row = MailSettingsRow::filter_by_id(1)
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    assert!(!row.encrypted_password.contains("isolated-smtp-secret"));
    assert!(store.cipher.decrypt(&row.encrypted_password).is_err());
    drop(connection);
    let mut edit = mail_input();
    edit.version = Some(saved.version);
    edit.password.clear();
    edit.enabled = false;
    let disabled = store
        .save_mail_settings(admin.id, edit.clone())
        .await
        .unwrap();
    assert!(disabled.password_configured);
    assert!(store.mail_delivery_settings().await.unwrap().is_none());
    assert!(
        store
            .save_mail_settings(admin.id, edit.clone())
            .await
            .is_err()
    );
    edit.version = Some(disabled.version);
    edit.enabled = true;
    let enabled = store
        .save_mail_settings(admin.id, edit.clone())
        .await
        .unwrap();
    let reopened = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("auth.sqlite3").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened
            .mail_delivery_settings()
            .await
            .unwrap()
            .unwrap()
            .password,
        "isolated-smtp-secret"
    );
    edit.version = Some(enabled.version);
    edit.password = "replacement-smtp-secret".into();
    let replaced = store
        .save_mail_settings(admin.id, edit.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .mail_delivery_settings()
            .await
            .unwrap()
            .unwrap()
            .password,
        "replacement-smtp-secret"
    );
    edit.version = Some(replaced.version);
    edit.username.clear();
    edit.password.clear();
    let anonymous = store.save_mail_settings(admin.id, edit).await.unwrap();
    assert!(!anonymous.password_configured);
    assert!(
        reopened
            .mail_delivery_settings()
            .await
            .unwrap()
            .unwrap()
            .password
            .is_empty()
    );
    drop(reopened);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn mail_settings_require_admin_and_validate_configuration() {
    let (store, directory) = database().await;
    let admin = store
        .bootstrap_admin("admin@example.test", PASSWORD)
        .await
        .unwrap();
    let member = register(&store, "member@example.test").await;
    assert!(
        store
            .save_mail_settings(member.id, mail_input())
            .await
            .is_err()
    );
    for (host, port, tls, from, username, password) in [
        (
            "https://smtp.example.test",
            587,
            "tls",
            "sender@example.test",
            "u",
            "p",
        ),
        (
            "smtp.example.test:587",
            587,
            "tls",
            "sender@example.test",
            "u",
            "p",
        ),
        (
            "smtp.example.test",
            0,
            "tls",
            "sender@example.test",
            "u",
            "p",
        ),
        (
            "smtp.example.test",
            587,
            "none",
            "sender@example.test",
            "u",
            "p",
        ),
        ("smtp.example.test", 587, "tls", "invalid", "u", "p"),
        (
            "smtp.example.test",
            587,
            "tls",
            "sender@example.test",
            "u",
            "",
        ),
        (
            "smtp.example.test",
            587,
            "tls",
            "sender@example.test",
            "",
            "p",
        ),
    ] {
        let mut input = mail_input();
        input.host = host.into();
        input.port = port;
        input.tls = tls.into();
        input.from = from.into();
        input.username = username.into();
        input.password = password.into();
        assert!(store.save_mail_settings(admin.id, input).await.is_err());
    }
    assert!(store.mail_settings().await.unwrap().is_none());
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

async fn database() -> (ProviderStore, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-auth-{}-{}",
        std::process::id(),
        token().unwrap()
    ));
    std::fs::create_dir(&directory).unwrap();
    let store = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("auth.sqlite3").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    (store, directory)
}

async fn register(store: &ProviderStore, email: &str) -> UserView {
    let code = store
        .issue_email_code(email, EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    store
        .register_user(email, &code.code, PASSWORD, "测试用户")
        .await
        .unwrap();
    store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.email == email)
        .unwrap()
}

#[tokio::test]
async fn registration_is_verified_normalized_and_never_promotes_admin() {
    let (store, directory) = database().await;
    assert!(!store.admin_initialized().await.unwrap());
    let code = store
        .issue_email_code(" Person+Tag@Example.TEST ", EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(code.email, "person+tag@example.test");
    assert!(store.login(&code.email, PASSWORD).await.unwrap().is_none());
    assert!(
        store
            .reset_password(&code.email, &code.code, PASSWORD)
            .await
            .is_err()
    );
    store
        .register_user("PERSON+TAG@example.test", &code.code, PASSWORD, " Person ")
        .await
        .unwrap();
    assert!(
        store
            .register_user(&code.email, &code.code, PASSWORD, "Replay")
            .await
            .is_err()
    );
    let user = store.list_users().await.unwrap().pop().unwrap();
    assert_eq!(user.role, UserRole::User);
    assert_eq!(user.display_name, "Person");
    assert!(user.email_verified);
    assert!(!store.admin_initialized().await.unwrap());
    let mut connection = store.connection().await.unwrap();
    let stored = UserRow::filter_by_id(user.id)
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.password_hash.starts_with("$argon2id$"));
    assert!(!stored.password_hash.contains(PASSWORD));
    let challenge = EmailChallengeRow::all()
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    assert!(!challenge.encrypted_code.contains(&code.code));
    drop(connection);
    let admin = store
        .bootstrap_admin("admin@example.test", PASSWORD)
        .await
        .unwrap();
    assert_eq!(admin.role, UserRole::Admin);
    assert!(
        store
            .bootstrap_admin("second@example.test", PASSWORD)
            .await
            .is_err()
    );
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn initial_admin_creation_is_atomic_across_connections() {
    let (store, directory) = database().await;
    let other = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("auth.sqlite3").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        store.bootstrap_admin("first@example.test", PASSWORD),
        other.bootstrap_admin("second@example.test", PASSWORD)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(store.list_users().await.unwrap().len(), 1);
    assert!(store.admin_initialized().await.unwrap());
    assert!(other.admin_initialized().await.unwrap());
    drop(other);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn email_codes_limit_attempts_expire_and_hide_unknown_accounts() {
    let (store, directory) = database().await;
    let code = store
        .issue_email_code("person@example.test", EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    assert!(
        store
            .issue_email_code(&code.email, EmailPurpose::Register)
            .await
            .is_err()
    );
    let wrong = if code.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    for _ in 0..5 {
        assert!(
            store
                .register_user(&code.email, wrong, PASSWORD, "Person")
                .await
                .is_err()
        );
    }
    assert!(
        store
            .register_user(&code.email, &code.code, PASSWORD, "Person")
            .await
            .is_err()
    );
    assert!(
        store
            .issue_email_code("missing@example.test", EmailPurpose::ResetPassword)
            .await
            .unwrap()
            .is_none()
    );
    let mut connection = store.connection().await.unwrap();
    let mut challenge = EmailChallengeRow::filter_by_scope("register:person@example.test")
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    challenge
        .update()
        .created_at(0_i64)
        .exec(&mut connection)
        .await
        .unwrap();
    drop(connection);
    let fresh = store
        .issue_email_code(&code.email, EmailPurpose::Register)
        .await
        .unwrap()
        .unwrap();
    let mut connection = store.connection().await.unwrap();
    let mut challenge = EmailChallengeRow::filter_by_scope("register:person@example.test")
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    challenge
        .update()
        .expires_at(0_i64)
        .exec(&mut connection)
        .await
        .unwrap();
    drop(connection);
    assert!(
        store
            .register_user(&code.email, &fresh.code, PASSWORD, "Person")
            .await
            .is_err()
    );
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn role_disable_password_and_logout_revoke_sessions() {
    let (store, directory) = database().await;
    let admin = store
        .bootstrap_admin("admin@example.test", PASSWORD)
        .await
        .unwrap();
    let user = register(&store, "person@example.test").await;
    assert!(
        store
            .update_user(
                user.id,
                admin.id,
                admin.version,
                "Admin",
                UserRole::User,
                false
            )
            .await
            .is_err()
    );
    assert!(
        store
            .update_user(
                admin.id,
                admin.id,
                admin.version,
                "Admin",
                UserRole::User,
                true
            )
            .await
            .is_err()
    );
    let first = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
    let second = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
    assert_ne!(first.secret, second.secret);
    assert_ne!(first.session.csrf, second.session.csrf);
    let current = store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.id == user.id)
        .unwrap();
    let promoted = store
        .update_user(
            admin.id,
            user.id,
            current.version,
            "Admin 2",
            UserRole::Admin,
            true,
        )
        .await
        .unwrap();
    assert!(
        store
            .authenticate_session(&first.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .authenticate_session(&second.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .update_user(
                admin.id,
                user.id,
                current.version,
                "stale",
                UserRole::User,
                true
            )
            .await
            .is_err()
    );
    store
        .update_user(
            admin.id,
            user.id,
            promoted.version,
            "Disabled",
            UserRole::User,
            false,
        )
        .await
        .unwrap();
    assert!(store.login(&user.email, PASSWORD).await.unwrap().is_none());
    let current = store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.id == user.id)
        .unwrap();
    store
        .update_user(
            admin.id,
            user.id,
            current.version,
            "Member",
            UserRole::User,
            true,
        )
        .await
        .unwrap();
    let first = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
    assert!(
        store
            .change_password(user.id, "wrong-password", NEW_PASSWORD)
            .await
            .is_err()
    );
    assert!(
        store
            .authenticate_session(&first.secret)
            .await
            .unwrap()
            .is_some()
    );
    store
        .change_password(user.id, PASSWORD, NEW_PASSWORD)
        .await
        .unwrap();
    assert!(
        store
            .authenticate_session(&first.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.login(&user.email, PASSWORD).await.unwrap().is_none());
    let session = store
        .login(&user.email, NEW_PASSWORD)
        .await
        .unwrap()
        .unwrap();
    store.logout(&session.secret).await.unwrap();
    assert!(
        store
            .authenticate_session(&session.secret)
            .await
            .unwrap()
            .is_none()
    );
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn password_recovery_revokes_all_devices_and_codes_are_single_use() {
    let (store, directory) = database().await;
    let user = register(&store, "person@example.test").await;
    let first = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
    let second = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
    let code = store
        .issue_email_code(&user.email, EmailPurpose::ResetPassword)
        .await
        .unwrap()
        .unwrap();
    store
        .reset_password(&user.email, &code.code, NEW_PASSWORD)
        .await
        .unwrap();
    assert!(
        store
            .reset_password(&user.email, &code.code, PASSWORD)
            .await
            .is_err()
    );
    for secret in [first.secret, second.secret] {
        assert!(store.authenticate_session(&secret).await.unwrap().is_none());
    }
    assert!(
        store
            .login(&user.email, NEW_PASSWORD)
            .await
            .unwrap()
            .is_some()
    );
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn sessions_have_absolute_idle_expiry_and_only_digests_are_stored() {
    let (store, directory) = database().await;
    let user = register(&store, "person@example.test").await;
    for idle in [false, true] {
        let session = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
        let mut connection = store.connection().await.unwrap();
        let mut row = UserSessionRow::filter_by_digest(digest(&session.secret))
            .first()
            .exec(&mut connection)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.digest, session.secret);
        if idle {
            row.update()
                .last_seen_at(0_i64)
                .exec(&mut connection)
                .await
                .unwrap();
        } else {
            row.update()
                .expires_at(0_i64)
                .exec(&mut connection)
                .await
                .unwrap();
        }
        drop(connection);
        assert!(
            store
                .authenticate_session(&session.secret)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(
        store
            .authenticate_session("malformed")
            .await
            .unwrap()
            .is_none()
    );
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn concurrent_connections_cannot_remove_all_admins() {
    let (store, directory) = database().await;
    let first = store
        .bootstrap_admin("first@example.test", PASSWORD)
        .await
        .unwrap();
    let second = register(&store, "second@example.test").await;
    let second = store
        .update_user(
            first.id,
            second.id,
            second.version,
            "Second Admin",
            UserRole::Admin,
            true,
        )
        .await
        .unwrap();
    let other = ProviderStore::connect(
        &format!("sqlite:{}", directory.join("auth.sqlite3").display()),
        &STANDARD.encode([7; 32]),
    )
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        store.update_user(
            first.id,
            first.id,
            first.version,
            "First",
            UserRole::User,
            true
        ),
        other.update_user(
            second.id,
            second.id,
            second.version,
            "Second",
            UserRole::User,
            true
        )
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        store
            .list_users()
            .await
            .unwrap()
            .iter()
            .filter(|u| u.role == UserRole::Admin && u.enabled)
            .count(),
        1
    );
    drop(other);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn background_revocation_checks_do_not_extend_idle_sessions() {
    let (store, directory) = database().await;
    let user = register(&store, "person@example.test").await;
    let session = store.login(&user.email, PASSWORD).await.unwrap().unwrap();
    let last_seen = now().unwrap() - 120;
    let mut connection = store.connection().await.unwrap();
    let mut row = UserSessionRow::filter_by_digest(digest(&session.secret))
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    row.update()
        .last_seen_at(last_seen)
        .exec(&mut connection)
        .await
        .unwrap();
    drop(connection);
    assert!(store.session_is_active(&session.secret).await.unwrap());
    let mut connection = store.connection().await.unwrap();
    let row = UserSessionRow::filter_by_digest(digest(&session.secret))
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.last_seen_at, last_seen);
    drop(connection);
    assert!(
        store
            .authenticate_session(&session.secret)
            .await
            .unwrap()
            .is_some()
    );
    let mut connection = store.connection().await.unwrap();
    let row = UserSessionRow::filter_by_digest(digest(&session.secret))
        .first()
        .exec(&mut connection)
        .await
        .unwrap()
        .unwrap();
    assert!(row.last_seen_at > last_seen);
    drop(connection);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}
