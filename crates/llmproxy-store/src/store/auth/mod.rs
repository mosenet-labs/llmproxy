use super::*;
use crate::{
    auth::{
        AuthSession, CreatedSession, EmailCode, EmailPurpose, UserRole, UserView, normalize_email,
    },
    model::{EmailChallengeRow, UserRow, UserSessionRow},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

mod challenges;
mod password;
mod sessions;
#[cfg(test)]
mod tests;
mod users;

const SESSION_LIFETIME: i64 = 7 * 24 * 3600;
const SESSION_IDLE: i64 = 24 * 3600;
const CODE_LIFETIME: i64 = 600;

fn validation(message: &str) -> StoreError {
    StoreError::Validation(message.to_owned())
}

fn token() -> StoreResult<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| StoreError::Internal)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn digest(secret: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(secret.as_bytes()))
}

fn user_view(row: UserRow) -> StoreResult<UserView> {
    Ok(UserView {
        id: row.id,
        email: row.email,
        display_name: row.display_name,
        role: match row.role.as_str() {
            "admin" => UserRole::Admin,
            "user" => UserRole::User,
            _ => return Err(StoreError::Internal),
        },
        enabled: row.enabled,
        email_verified: row.email_verified,
        created_at: row.created_at,
        last_login_at: row.last_login_at,
        version: row.version,
    })
}

fn display_name(input: &str) -> StoreResult<String> {
    let name = input.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(validation("显示名称须为 1–80 个字符"));
    }
    Ok(name.to_owned())
}

async fn find_user(tx: &mut dyn Executor, id: i64) -> StoreResult<UserRow> {
    UserRow::filter_by_id(id)
        .first()
        .exec(tx)
        .await?
        .ok_or_else(|| validation("用户不存在或已被删除"))
}

pub(super) async fn require_admin(tx: &mut dyn Executor, id: i64) -> StoreResult<()> {
    let user = find_user(tx, id).await?;
    if !user.enabled || !user.email_verified || user.role != "admin" {
        return Err(validation("需要平台管理员权限"));
    }
    Ok(())
}

async fn revoke_sessions(tx: &mut dyn Executor, user_id: i64) -> StoreResult<()> {
    UserSessionRow::all()
        .filter(UserSessionRow::fields().user_id().eq(user_id))
        .delete()
        .exec(tx)
        .await?;
    Ok(())
}
