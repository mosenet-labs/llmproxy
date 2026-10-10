use super::*;

impl ProviderStore {
    pub async fn login(&self, email: &str, password: &str) -> StoreResult<Option<CreatedSession>> {
        let email = normalize_email(email)?;
        let user = {
            let mut connection = self.connection().await?;
            UserRow::filter_by_email(email)
                .first()
                .exec(&mut connection)
                .await?
        };
        if !password::verify(password, user.as_ref().map(|u| u.password_hash.as_str())).await? {
            return Ok(None);
        }
        let Some(user) = user else { return Ok(None) };
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut user_now = find_user(&mut tx, user.id).await?;
        if !user_now.enabled
            || !user_now.email_verified
            || user_now.password_hash != user.password_hash
        {
            return Ok(None);
        }
        let time = now()?;
        let secret = format!("lp-us-{}", token()?);
        let csrf = token()?;
        let expires_at = time + SESSION_LIFETIME;
        UserSessionRow::all()
            .filter(UserSessionRow::fields().expires_at().le(time))
            .delete()
            .exec(&mut tx)
            .await?;
        UserSessionRow::create()
            .digest(digest(&secret))
            .user_id(user.id)
            .csrf(&csrf)
            .created_at(time)
            .last_seen_at(time)
            .expires_at(expires_at)
            .exec(&mut tx)
            .await?;
        user_now
            .update()
            .last_login_at(Some(time))
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(Some(CreatedSession {
            secret,
            session: AuthSession {
                user: user_view(user_now)?,
                csrf,
                expires_at,
            },
        }))
    }

    pub async fn authenticate_session(&self, secret: &str) -> StoreResult<Option<AuthSession>> {
        self.check_session(secret, true).await
    }

    /// Background revocation checks must not extend an idle browser session.
    pub async fn session_is_active(&self, secret: &str) -> StoreResult<bool> {
        Ok(self.check_session(secret, false).await?.is_some())
    }

    async fn check_session(&self, secret: &str, touch: bool) -> StoreResult<Option<AuthSession>> {
        if !secret.starts_with("lp-us-") || secret.len() != 49 {
            return Ok(None);
        }
        let mut connection = self.connection().await?;
        let Some(mut row) = UserSessionRow::filter_by_digest(digest(secret))
            .first()
            .exec(&mut connection)
            .await?
        else {
            return Ok(None);
        };
        let time = now()?;
        if row.expires_at <= time || row.last_seen_at + SESSION_IDLE <= time {
            return Ok(None);
        }
        let user = find_user(&mut connection, row.user_id).await?;
        if !user.enabled || !user.email_verified {
            return Ok(None);
        }
        if touch && row.last_seen_at + 60 <= time {
            row.update()
                .last_seen_at(time)
                .exec(&mut connection)
                .await?;
        }
        Ok(Some(AuthSession {
            user: user_view(user)?,
            csrf: row.csrf,
            expires_at: row.expires_at,
        }))
    }

    pub async fn logout(&self, secret: &str) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        UserSessionRow::filter_by_digest(digest(secret))
            .delete()
            .exec(&mut connection)
            .await?;
        Ok(())
    }
}
