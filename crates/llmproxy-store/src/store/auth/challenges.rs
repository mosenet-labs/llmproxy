use super::*;

impl ProviderStore {
    pub async fn issue_email_code(
        &self,
        email: &str,
        purpose: EmailPurpose,
    ) -> StoreResult<Option<EmailCode>> {
        let email = normalize_email(email)?;
        let scope = format!("{}:{email}", purpose.as_str());
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let time = now()?;
        let previous = EmailChallengeRow::filter_by_scope(&scope)
            .first()
            .exec(&mut tx)
            .await?;
        if previous.as_ref().is_some_and(|c| c.created_at + 60 > time) {
            return Err(validation("发送过于频繁，请稍后再试"));
        }
        let user = UserRow::filter_by_email(&email)
            .first()
            .exec(&mut tx)
            .await?;
        let send = match (&user, purpose) {
            (None, EmailPurpose::Register) => {
                UserRow::create()
                    .email(&email)
                    .display_name(&email)
                    .password_hash("")
                    .role("user")
                    .enabled(true)
                    .email_verified(false)
                    .created_at(time)
                    .updated_at(time)
                    .exec(&mut tx)
                    .await?;
                true
            }
            (Some(u), EmailPurpose::Register) => !u.email_verified && u.enabled,
            (Some(u), EmailPurpose::ResetPassword) => u.email_verified && u.enabled,
            _ => false,
        };
        let code = random_code()?;
        let encrypted = self.cipher.encrypt_bound(&code, scope.as_bytes())?;
        if let Some(mut previous) = previous {
            previous
                .update()
                .encrypted_code(encrypted)
                .created_at(time)
                .expires_at(time + CODE_LIFETIME)
                .attempts(0)
                .used(!send)
                .exec(&mut tx)
                .await?;
        } else {
            EmailChallengeRow::create()
                .scope(&scope)
                .encrypted_code(encrypted)
                .created_at(time)
                .expires_at(time + CODE_LIFETIME)
                .attempts(0)
                .used(!send)
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(send.then_some(EmailCode {
            email,
            code,
            purpose,
        }))
    }

    pub async fn register_user(
        &self,
        email: &str,
        code: &str,
        password: &str,
        name: &str,
    ) -> StoreResult<()> {
        let email = normalize_email(email)?;
        let name = if name.trim().is_empty() {
            email.clone()
        } else {
            display_name(name)?
        };
        let hash = password::hash(password).await?;
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        if !self
            .consume_code(&mut tx, &email, EmailPurpose::Register, code)
            .await?
        {
            tx.commit().await?;
            return Err(validation("验证码无效、已过期或尝试次数过多"));
        }
        let Some(mut user) = UserRow::filter_by_email(&email)
            .first()
            .exec(&mut tx)
            .await?
        else {
            return Err(validation("注册状态已失效，请重新发送验证码"));
        };
        if user.email_verified || !user.enabled {
            return Err(validation("账户无法注册，请登录或联系管理员"));
        }
        user.update()
            .password_hash(hash)
            .display_name(name)
            .email_verified(true)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn reset_password(&self, email: &str, code: &str, password: &str) -> StoreResult<()> {
        let email = normalize_email(email)?;
        let hash = password::hash(password).await?;
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        if !self
            .consume_code(&mut tx, &email, EmailPurpose::ResetPassword, code)
            .await?
        {
            tx.commit().await?;
            return Err(validation("验证码无效、已过期或尝试次数过多"));
        }
        let Some(mut user) = UserRow::filter_by_email(&email)
            .first()
            .exec(&mut tx)
            .await?
        else {
            return Err(validation("账户无法重置密码"));
        };
        if !user.enabled || !user.email_verified {
            return Err(validation("账户无法重置密码"));
        }
        user.update()
            .password_hash(hash)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        revoke_sessions(&mut tx, user.id).await?;
        // Invalidate outstanding codes so they cannot regain access after recovery.
        EmailChallengeRow::filter_by_scope(format!("register:{email}"))
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn consume_code(
        &self,
        tx: &mut dyn Executor,
        email: &str,
        purpose: EmailPurpose,
        input: &str,
    ) -> StoreResult<bool> {
        let scope = format!("{}:{email}", purpose.as_str());
        let Some(mut row) = EmailChallengeRow::filter_by_scope(&scope)
            .first()
            .exec(tx)
            .await?
        else {
            return Ok(false);
        };
        if row.used || row.expires_at <= now()? || row.attempts >= 5 {
            return Ok(false);
        }
        let expected = self
            .cipher
            .decrypt_bound(&row.encrypted_code, scope.as_bytes())?;
        let valid = input.len() == 6 && bool::from(expected.as_bytes().ct_eq(input.as_bytes()));
        let attempts = row.attempts + 1;
        row.update().attempts(attempts).used(valid).exec(tx).await?;
        Ok(valid)
    }
}

fn random_code() -> StoreResult<String> {
    loop {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).map_err(|_| StoreError::Internal)?;
        let number = u32::from_le_bytes(bytes);
        if number < u32::MAX - u32::MAX % 1_000_000 {
            return Ok(format!("{:06}", number % 1_000_000));
        }
    }
}
