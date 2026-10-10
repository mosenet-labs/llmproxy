use super::*;

async fn invitation_view(
    tx: &mut dyn Executor,
    row: OrganizationInvitationRow,
) -> StoreResult<OrganizationInvitation> {
    let user = UserRow::filter_by_id(row.user_id).get(&mut *tx).await?;
    let space = ResourceSpaceRow::filter_by_id(row.space_id).get(tx).await?;
    Ok(OrganizationInvitation {
        id: row.id,
        space_id: row.space_id,
        space_name: space.name,
        email: user.email,
        role: OrganizationRole::parse(&row.role)?,
        expires_at: row.expires_at,
        version: row.version,
    })
}

impl ProviderStore {
    /// Invitations are addressed to existing accounts and accepted by that account.
    pub async fn invite_organization_member(
        &self,
        actor: i64,
        space: i64,
        email: &str,
        role: OrganizationRole,
    ) -> StoreResult<()> {
        let email = crate::auth::normalize_email(email)?;
        if role == OrganizationRole::Owner {
            return Err(StoreError::Validation("不能邀请为所有者".into()));
        }
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        let organization = owner(&mut tx, actor, space).await?;
        if !organization.enabled {
            return Err(StoreError::Validation("请先启用组织".into()));
        }
        let user = UserRow::all()
            .filter(UserRow::fields().email().eq(email))
            .first()
            .exec(&mut tx)
            .await?
            .ok_or_else(|| StoreError::Validation("该邮箱尚未注册，请先注册账号".into()))?;
        active_user(&mut tx, user.id).await?;
        if OrganizationMemberRow::all()
            .filter(OrganizationMemberRow::fields().space_id().eq(space))
            .filter(OrganizationMemberRow::fields().user_id().eq(user.id))
            .first()
            .exec(&mut tx)
            .await?
            .is_some()
        {
            return Err(StoreError::Conflict("该用户已是组织成员".into()));
        }
        if let Some(mut old) = OrganizationInvitationRow::all()
            .filter(OrganizationInvitationRow::fields().space_id().eq(space))
            .filter(OrganizationInvitationRow::fields().user_id().eq(user.id))
            .filter(OrganizationInvitationRow::fields().status().eq("pending"))
            .first()
            .exec(&mut tx)
            .await?
        {
            if old.expires_at > now()? {
                return Err(StoreError::Conflict("该用户已有待接受的邀请".into()));
            }
            old.update().status("revoked").exec(&mut tx).await?;
        }
        OrganizationInvitationRow::create()
            .space_id(space)
            .user_id(user.id)
            .role(role.as_str())
            .status("pending")
            .expires_at(now()? + 7 * 86400)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn organization_invitations(
        &self,
        actor: i64,
        space: Option<i64>,
    ) -> StoreResult<Vec<OrganizationInvitation>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        active_user(&mut tx, actor).await?;
        let mut query = OrganizationInvitationRow::all()
            .filter(OrganizationInvitationRow::fields().status().eq("pending"))
            .filter(OrganizationInvitationRow::fields().expires_at().gt(now()?));
        if let Some(space) = space {
            owner(&mut tx, actor, space).await?;
            query = query.filter(OrganizationInvitationRow::fields().space_id().eq(space));
        } else {
            query = query.filter(OrganizationInvitationRow::fields().user_id().eq(actor));
        }
        let mut result = Vec::new();
        for row in query
            .order_by(OrganizationInvitationRow::fields().id().asc())
            .exec(&mut tx)
            .await?
        {
            result.push(invitation_view(&mut tx, row).await?);
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn accept_organization_invitation(
        &self,
        actor: i64,
        id: i64,
        expected: u64,
    ) -> StoreResult<()> {
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        active_user(&mut tx, actor).await?;
        let mut row = OrganizationInvitationRow::filter_by_id(id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        version(row.version, expected)?;
        if row.user_id != actor || row.status != "pending" || row.expires_at <= now()? {
            return Err(StoreError::NotFound);
        }
        let space = ResourceSpaceRow::filter_by_id(row.space_id)
            .get(&mut tx)
            .await?;
        if !space.enabled {
            return Err(StoreError::Validation("组织已停用".into()));
        }
        if OrganizationMemberRow::all()
            .filter(OrganizationMemberRow::fields().space_id().eq(row.space_id))
            .filter(OrganizationMemberRow::fields().user_id().eq(actor))
            .first()
            .exec(&mut tx)
            .await?
            .is_some()
        {
            return Err(StoreError::Conflict("已加入该组织".into()));
        }
        OrganizationMemberRow::create()
            .space_id(row.space_id)
            .user_id(actor)
            .role(row.role.clone())
            .exec(&mut tx)
            .await?;
        row.update().status("accepted").exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn revoke_organization_invitation(
        &self,
        actor: i64,
        space: i64,
        id: i64,
        expected: u64,
    ) -> StoreResult<()> {
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        owner(&mut tx, actor, space).await?;
        let mut row = OrganizationInvitationRow::filter_by_id(id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        version(row.version, expected)?;
        if row.space_id != space || row.status != "pending" {
            return Err(StoreError::NotFound);
        }
        row.update().status("revoked").exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
