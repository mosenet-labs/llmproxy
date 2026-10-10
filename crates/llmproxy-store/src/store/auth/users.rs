use super::*;

impl ProviderStore {
    pub async fn admin_initialized(&self) -> StoreResult<bool> {
        let mut connection = self.connection().await?;
        Ok(UserRow::all()
            .filter(UserRow::fields().role().eq("admin"))
            .first()
            .exec(&mut connection)
            .await?
            .is_some())
    }

    pub async fn bootstrap_admin(&self, email: &str, password: &str) -> StoreResult<UserView> {
        let email = normalize_email(email)?;
        let hash = password::hash(password).await?;
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        if UserRow::all()
            .filter(UserRow::fields().role().eq("admin"))
            .first()
            .exec(&mut tx)
            .await?
            .is_some()
        {
            return Err(validation("管理员已初始化，请通过用户管理调整权限"));
        }
        let time = now()?;
        let row = if let Some(mut row) = UserRow::filter_by_email(&email)
            .first()
            .exec(&mut tx)
            .await?
        {
            row.update()
                .password_hash(hash)
                .role("admin")
                .enabled(true)
                .email_verified(true)
                .updated_at(time)
                .exec(&mut tx)
                .await?;
            revoke_sessions(&mut tx, row.id).await?;
            row
        } else {
            UserRow::create()
                .email(&email)
                .display_name("管理员")
                .password_hash(hash)
                .role("admin")
                .enabled(true)
                .email_verified(true)
                .created_at(time)
                .updated_at(time)
                .exec(&mut tx)
                .await?
        };
        tx.commit().await?;
        user_view(row)
    }

    pub async fn get_user(&self, id: i64) -> StoreResult<UserView> {
        let mut connection = self.connection().await?;
        user_view(find_user(&mut connection, id).await?)
    }

    pub async fn list_users(&self) -> StoreResult<Vec<UserView>> {
        let mut connection = self.connection().await?;
        UserRow::all()
            .order_by(UserRow::fields().id().asc())
            .exec(&mut connection)
            .await?
            .into_iter()
            .map(user_view)
            .collect()
    }

    pub async fn update_user(
        &self,
        actor_id: i64,
        id: i64,
        version: u64,
        name: &str,
        role: UserRole,
        enabled: bool,
    ) -> StoreResult<UserView> {
        let name = display_name(name)?;
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        require_admin(&mut tx, actor_id).await?;
        let mut row = find_user(&mut tx, id).await?;
        if row.version != version {
            return Err(validation("用户已被修改，请刷新后重试"));
        }
        if role == UserRole::Admin && !row.email_verified {
            return Err(validation("未验证邮箱的用户不能设为管理员"));
        }
        if row.role == "admin" && row.enabled && (!enabled || role != UserRole::Admin) {
            let admins = UserRow::all()
                .filter(UserRow::fields().role().eq("admin"))
                .filter(UserRow::fields().enabled().eq(true))
                .exec(&mut tx)
                .await?;
            if admins.len() <= 1 {
                return Err(validation("不能禁用或降级最后一个有效管理员"));
            }
        }
        let changed_access = row.role != role.as_str() || row.enabled != enabled;
        row.update()
            .display_name(name)
            .role(role.as_str())
            .enabled(enabled)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        if changed_access {
            revoke_sessions(&mut tx, id).await?;
        }
        tx.commit().await?;
        user_view(row)
    }

    pub async fn revoke_user_sessions(&self, actor_id: i64, id: i64) -> StoreResult<()> {
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        require_admin(&mut tx, actor_id).await?;
        find_user(&mut tx, id).await?;
        revoke_sessions(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_profile(&self, id: i64, name: &str) -> StoreResult<()> {
        let name = display_name(name)?;
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut row = find_user(&mut tx, id).await?;
        if !row.enabled || !row.email_verified {
            return Err(validation("账户不可用"));
        }
        row.update()
            .display_name(name)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn change_password(&self, id: i64, current: &str, new: &str) -> StoreResult<()> {
        let old_hash = {
            let mut connection = self.connection().await?;
            find_user(&mut connection, id).await?.password_hash
        };
        if !password::verify(current, Some(&old_hash)).await? {
            return Err(validation("当前密码不正确"));
        }
        let new_hash = password::hash(new).await?;
        let _writes = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut row = find_user(&mut tx, id).await?;
        if !row.enabled || !row.email_verified || row.password_hash != old_hash {
            return Err(validation("账户状态已变化，请重新登录"));
        }
        row.update()
            .password_hash(new_hash)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        revoke_sessions(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }
}
