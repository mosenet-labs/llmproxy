use crate::app::{AppState, auth, check_csrf};
use llmproxy_store::{StoreError, auth::UserRole, settings::MailSettingsInput};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::content::Form,
    runtime::procedure,
};

#[derive(Deserialize)]
struct MailForm {
    csrf: String,
    #[serde(default)]
    enabled: bool,
    host: String,
    port: u16,
    tls: String,
    from: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    version: Option<u64>,
}

fn authorize(cx: &Cx, csrf: &str) -> Result<i64> {
    check_csrf(cx, csrf)?;
    let identity = auth::session(cx)?;
    if identity.user.role != UserRole::Admin {
        return Err(topcoat::router::error::forbidden().into());
    }
    Ok(identity.user.id)
}

#[procedure("/ui/_topcoat/runtime/procedures/save-mail-settings")]
pub async fn save_mail_settings(cx: &Cx, payload: String) -> Result<auth::Outcome> {
    let Form(input) = Form::<MailForm>::from_bytes(payload.as_bytes())?;
    let actor = authorize(cx, &input.csrf)?;
    Ok(app_context::<AppState>(cx)
        .store
        .save_mail_settings(
            actor,
            MailSettingsInput {
                enabled: input.enabled,
                host: input.host,
                port: input.port,
                tls: input.tls,
                from: input.from,
                username: input.username,
                password: input.password,
                version: input.version,
            },
        )
        .await
        .map(|_| "邮件配置已保存，立即生效".into())
        .map_err(|error| match error {
            StoreError::Validation(message) | StoreError::Conflict(message) => message,
            _ => "邮件配置保存失败，请稍后重试".into(),
        }))
}

#[derive(Deserialize)]
struct TestMailForm {
    csrf: String,
    recipient: String,
}

#[procedure("/ui/_topcoat/runtime/procedures/test-mail-settings")]
pub async fn test_mail_settings(cx: &Cx, payload: String) -> Result<auth::Outcome> {
    let Form(input) = Form::<TestMailForm>::from_bytes(payload.as_bytes())?;
    authorize(cx, &input.csrf)?;
    let recipient = match llmproxy_store::auth::normalize_email(&input.recipient) {
        Ok(email) => email,
        Err(error) => return Ok(Err(auth::message(error))),
    };
    let Some(config) = app_context::<AppState>(cx)
        .store
        .mail_delivery_settings()
        .await?
    else {
        return Ok(Err("请先保存并启用邮件服务".into()));
    };
    let mailer = match auth::mail::Mailer::new(config) {
        Ok(mailer) => mailer,
        Err(_) => return Ok(Err("邮件服务配置无效，请检查配置后重试".into())),
    };
    Ok(mailer
        .send_test(&recipient)
        .await
        .map(|_| "测试邮件已发送，请检查收件箱及垃圾邮件".into()))
}
