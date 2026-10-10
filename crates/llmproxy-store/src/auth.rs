//! Public account views never expose passwords, session secrets or email codes.

use serde::{Deserialize, Serialize};

pub fn normalize_email(input: &str) -> crate::StoreResult<String> {
    let email = input.trim().to_lowercase();
    if email.len() > 254 || !email_address::EmailAddress::is_valid(&email) {
        return Err(crate::StoreError::Validation("请输入有效的邮箱地址".into()));
    }
    Ok(email)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Admin,
    User,
}

impl UserRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UserView {
    pub id: i64,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub enabled: bool,
    pub email_verified: bool,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    pub version: u64,
}

#[derive(Clone)]
pub struct AuthSession {
    pub user: UserView,
    pub csrf: String,
    pub expires_at: i64,
}

pub struct CreatedSession {
    pub secret: String,
    pub session: AuthSession,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum EmailPurpose {
    Register,
    ResetPassword,
}

impl EmailPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::ResetPassword => "reset_password",
        }
    }
}

pub struct EmailCode {
    pub email: String,
    pub code: String,
    pub purpose: EmailPurpose,
}
