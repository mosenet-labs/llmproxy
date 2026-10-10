//! Platform settings. Public views contain no SMTP credentials.

#[derive(Clone, Debug)]
pub struct MailSettingsView {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub from: String,
    pub username: String,
    pub password_configured: bool,
    pub version: u64,
}

/// Write-only password; an empty value preserves the saved password.
#[derive(Clone)]
pub struct MailSettingsInput {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub from: String,
    pub username: String,
    pub password: String,
    pub version: Option<u64>,
}

/// Internal delivery configuration; deliberately not Debug or Serialize.
pub struct MailDeliverySettings {
    pub settings: MailSettingsView,
    pub password: String,
}
