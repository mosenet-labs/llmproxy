use super::*;
use llmproxy_store::auth::{CreatedSession, EmailPurpose};
use serde::Deserialize;
use topcoat::{router::content::Form, runtime::procedure};

#[derive(Deserialize)]
pub(crate) struct AuthForm {
    csrf: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    confirmation: String,
    #[serde(default)]
    current_password: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    purpose: String,
}

fn parse(cx: &Cx, payload: &str) -> Result<AuthForm> {
    let Form(input) = Form::<AuthForm>::from_bytes(payload.as_bytes())?;
    super::super::check_csrf(cx, &input.csrf)?;
    Ok(input)
}

fn confirmed(input: &AuthForm) -> std::result::Result<(), String> {
    if input.password != input.confirmation {
        return Err("两次输入的密码不一致".into());
    }
    Ok(())
}

async fn start_session(cx: &Cx, created: CreatedSession) -> Result<Outcome> {
    if let Some(old) = session_secret(headers(cx)) {
        app_context::<AppState>(cx).store.logout(old).await?;
    }
    set_cookie(cx, &created.secret, 7 * 24 * 3600)?;
    Ok(Ok(if created.session.user.role == UserRole::Admin {
        "/ui/chat"
    } else {
        "/ui/account"
    }
    .into()))
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-bootstrap-admin")]
pub async fn bootstrap_admin(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    let state = app_context::<AppState>(cx);
    if state.store.admin_initialized().await? {
        return Ok(Err("管理员已初始化，请使用已有账户登录".into()));
    }
    if let Err(error) = confirmed(&input).and_then(|_| {
        state
            .auth
            .attempt(cx, "bootstrap-admin", &input.email, false)
    }) {
        return Ok(Err(error));
    }
    if let Err(error) = state
        .store
        .bootstrap_admin(&input.email, &input.password)
        .await
    {
        return Ok(Err(message(error)));
    }
    match state.store.login(&input.email, &input.password).await? {
        Some(created) => start_session(cx, created).await,
        None => Ok(Err("管理员已创建，请使用邮箱和密码登录".into())),
    }
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-login")]
pub async fn login(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    let state = app_context::<AppState>(cx);
    if let Err(error) = state.auth.attempt(cx, "login", &input.email, false) {
        return Ok(Err(error));
    }
    match state.store.login(&input.email, &input.password).await {
        Ok(Some(created)) => start_session(cx, created).await,
        Ok(None) => Ok(Err("邮箱或密码不正确，或账户尚未验证/已禁用".into())),
        Err(error) => Ok(Err(message(error))),
    }
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-send-code")]
pub async fn send_code(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    let purpose = match input.purpose.as_str() {
        "register" => EmailPurpose::Register,
        "reset_password" => EmailPurpose::ResetPassword,
        _ => return Ok(Err("请选择有效的验证用途".into())),
    };
    Ok(deliver_code(cx, &input.email, purpose).await)
}

pub(crate) async fn deliver_code(cx: &Cx, email: &str, purpose: EmailPurpose) -> Outcome {
    let state = app_context::<AppState>(cx);
    let config = state
        .store
        .mail_delivery_settings()
        .await
        .map_err(message)?
        .ok_or_else(|| "未配置邮件服务，暂时无法发送验证邮件".to_owned())?;
    let mailer =
        mail::Mailer::new(config).map_err(|_| "邮件服务配置无效，请联系管理员".to_owned())?;
    state.auth.attempt(cx, purpose.as_str(), email, true)?;
    if let Some(code) = state
        .store
        .issue_email_code(email, purpose)
        .await
        .map_err(message)?
    {
        mailer.send(code).await?;
    }
    Ok("如果邮箱符合条件，验证码已发送，请检查收件箱及垃圾邮件".into())
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-register")]
pub async fn register(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    let state = app_context::<AppState>(cx);
    if !state.store.mail_enabled().await? {
        return Ok(Err("未配置邮件服务，暂时关闭注册".into()));
    }
    if let Err(error) =
        confirmed(&input).and_then(|_| state.auth.attempt(cx, "verify", &input.email, false))
    {
        return Ok(Err(error));
    }
    Ok(state
        .store
        .register_user(&input.email, &input.code, &input.password, "")
        .await
        .map(|_| "注册成功，请使用邮箱和密码登录".into())
        .map_err(message))
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-reset-password")]
pub async fn reset_password(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    let state = app_context::<AppState>(cx);
    if !state.store.mail_enabled().await? {
        return Ok(Err("未配置邮件服务，暂时无法找回密码".into()));
    }
    if let Err(error) =
        confirmed(&input).and_then(|_| state.auth.attempt(cx, "verify", &input.email, false))
    {
        return Ok(Err(error));
    }
    Ok(state
        .store
        .reset_password(&input.email, &input.code, &input.password)
        .await
        .map(|_| "密码已重置，旧登录会话已失效，请重新登录".into())
        .map_err(message))
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-logout")]
pub async fn logout(cx: &Cx, payload: String) -> Result<Outcome> {
    parse(cx, &payload)?;
    if let Some(secret) = session_secret(headers(cx)) {
        app_context::<AppState>(cx).store.logout(secret).await?;
    }
    set_cookie(cx, "", 0)?;
    Ok(Ok("/ui/login".into()))
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-save-profile")]
pub async fn save_profile(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    Ok(app_context::<AppState>(cx)
        .store
        .update_profile(session(cx)?.user.id, &input.display_name)
        .await
        .map(|_| "个人信息已保存".into())
        .map_err(message))
}

#[procedure("/ui/_topcoat/runtime/procedures/auth-change-password")]
pub async fn change_password(cx: &Cx, payload: String) -> Result<Outcome> {
    let input = parse(cx, &payload)?;
    let state = app_context::<AppState>(cx);
    if let Err(error) = confirmed(&input).and_then(|_| {
        state.auth.attempt(
            cx,
            "change-password",
            &session(cx).map_err(|_| "请重新登录".to_owned())?.user.email,
            false,
        )
    }) {
        return Ok(Err(error));
    }
    Ok(state
        .store
        .change_password(
            session(cx)?.user.id,
            &input.current_password,
            &input.password,
        )
        .await
        .map(|_| "/ui/login".into())
        .map_err(message))
}
