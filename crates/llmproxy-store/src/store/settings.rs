use super::auth::require_admin;
use super::*;
use crate::auth::normalize_email;
use crate::{
    model::MailSettingsRow,
    settings::{MailDeliverySettings, MailSettingsInput, MailSettingsView},
};

fn invalid(message: &str) -> StoreError {
    StoreError::Validation(message.to_owned())
}

const PASSWORD_AAD: &[u8] = b"llmproxy.mail-settings.password.v1";

fn view(row: MailSettingsRow) -> MailSettingsView {
    MailSettingsView {
        enabled: row.enabled,
        host: row.host,
        port: row.port,
        tls: row.tls,
        from: row.sender_email,
        username: row.username,
        password_configured: !row.encrypted_password.is_empty(),
        version: row.version,
    }
}

impl ProviderStore {
    pub async fn mail_settings(&self) -> StoreResult<Option<MailSettingsView>> {
        let mut connection = self.connection().await?;
        Ok(MailSettingsRow::filter_by_id(1)
            .first()
            .exec(&mut connection)
            .await?
            .map(view))
    }

    pub async fn mail_enabled(&self) -> StoreResult<bool> {
        Ok(self.mail_settings().await?.is_some_and(|s| s.enabled))
    }

    pub async fn mail_delivery_settings(&self) -> StoreResult<Option<MailDeliverySettings>> {
        let mut connection = self.connection().await?;
        let Some(row) = MailSettingsRow::filter_by_id(1)
            .first()
            .exec(&mut connection)
            .await?
            .filter(|s| s.enabled)
        else {
            return Ok(None);
        };
        let password = if row.encrypted_password.is_empty() {
            String::new()
        } else {
            self.cipher
                .decrypt_bound(&row.encrypted_password, PASSWORD_AAD)?
        };
        Ok(Some(MailDeliverySettings {
            settings: view(row),
            password,
        }))
    }

    pub async fn save_mail_settings(
        &self,
        actor_id: i64,
        input: MailSettingsInput,
    ) -> StoreResult<MailSettingsView> {
        let host = input.host.trim();
        let valid_host = host.parse::<std::net::IpAddr>().is_ok()
            || host.split('.').all(|part| {
                !part.is_empty()
                    && part.len() <= 63
                    && !part.starts_with('-')
                    && !part.ends_with('-')
                    && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            });
        if host.len() > 253 || !valid_host {
            return Err(invalid("SMTP 服务器须为主机名或 IP，不包含协议或路径"));
        }
        if input.port == 0 {
            return Err(invalid("SMTP 端口须为 1–65535"));
        }
        match input.tls.as_str() {
            "starttls" | "tls" => {}
            "none"
                if host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback()) => {}
            _ => {
                return Err(invalid(
                    "外部 SMTP 必须使用 STARTTLS 或 TLS；无加密仅限本机测试",
                ));
            }
        }
        let from = normalize_email(&input.from)?;
        let username = input.username.trim();
        if username.len() > 254
            || username.chars().any(char::is_control)
            || input.password.len() > 1024
        {
            return Err(invalid("SMTP 认证信息无效或过长"));
        }
        if username.is_empty() && !input.password.is_empty() {
            return Err(invalid("填写 SMTP 密码时必须填写用户名"));
        }
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        require_admin(&mut tx, actor_id).await?;
        let existing = MailSettingsRow::filter_by_id(1)
            .first()
            .exec(&mut tx)
            .await?;
        if existing.as_ref().map(|r| r.version) != input.version {
            return Err(invalid("邮件配置已被修改，请刷新后重试"));
        }
        let encrypted_password = if username.is_empty() {
            String::new()
        } else if input.password.is_empty() {
            existing
                .as_ref()
                .map(|r| r.encrypted_password.clone())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("请填写 SMTP 密码"))?
        } else {
            self.cipher.encrypt_bound(&input.password, PASSWORD_AAD)?
        };
        let row = if let Some(mut row) = existing {
            row.update()
                .enabled(input.enabled)
                .host(host)
                .port(input.port)
                .tls(input.tls)
                .sender_email(from)
                .username(username)
                .encrypted_password(encrypted_password)
                .exec(&mut tx)
                .await?;
            row
        } else {
            MailSettingsRow::create()
                .id(1)
                .enabled(input.enabled)
                .host(host)
                .port(input.port)
                .tls(input.tls)
                .sender_email(from)
                .username(username)
                .encrypted_password(encrypted_password)
                .exec(&mut tx)
                .await?
        };
        tx.commit().await?;
        Ok(view(row))
    }
}
