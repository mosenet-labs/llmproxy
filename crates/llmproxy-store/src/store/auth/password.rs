use super::*;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use tokio::sync::Semaphore;

static HASH_SLOTS: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(2)));

pub(super) async fn hash(password: &str) -> StoreResult<String> {
    if !(15..=128).contains(&password.chars().count()) {
        return Err(validation("密码须为 15–128 个字符"));
    }
    let password = password.to_owned();
    let permit = HASH_SLOTS
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| StoreError::Internal)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|_| StoreError::Internal)
    })
    .await
    .map_err(|_| StoreError::Internal)?
}

pub(super) async fn verify(password: &str, hash: Option<&str>) -> StoreResult<bool> {
    if password.len() > 512 {
        return Ok(false);
    }
    let password = password.to_owned();
    let hash = hash.filter(|h| !h.is_empty()).map(str::to_owned);
    let permit = HASH_SLOTS
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| StoreError::Internal)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if let Some(hash) = hash {
            let parsed = PasswordHash::new(&hash).map_err(|_| StoreError::Internal)?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        } else {
            // Unknown accounts do the same costly work as a password check.
            Argon2::default()
                .hash_password(password.as_bytes())
                .map_err(|_| StoreError::Internal)?;
            Ok(false)
        }
    })
    .await
    .map_err(|_| StoreError::Internal)?
}
