use super::*;

async fn member(tx: &mut dyn Executor, space: i64, id: i64) -> StoreResult<OrganizationMemberRow> {
    let row = OrganizationMemberRow::filter_by_id(id)
        .first()
        .exec(tx)
        .await?
        .ok_or(StoreError::NotFound)?;
    if row.space_id != space {
        return Err(StoreError::NotFound);
    }
    Ok(row)
}

async fn clear_access(tx: &mut dyn Executor, space: i64, user: i64) -> StoreResult<()> {
    let groups = GroupRow::all()
        .filter(GroupRow::fields().space_id().eq(space))
        .select(GroupRow::fields().id())
        .exec(&mut *tx)
        .await?;
    GroupAccessRow::all()
        .filter(GroupAccessRow::fields().user_id().eq(user))
        .filter(GroupAccessRow::fields().group_id().in_list(groups))
        .delete()
        .exec(tx)
        .await?;
    Ok(())
}

impl ProviderStore {
    pub async fn organization_members(
        &self,
        actor: i64,
        space: i64,
    ) -> StoreResult<Vec<OrganizationMember>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let (row, role, _) = Self::space_access(&mut tx, actor, space).await?;
        if row.kind != "organization" || !role.can_manage() {
            return Err(StoreError::NotFound);
        }
        let groups = GroupRow::all()
            .filter(GroupRow::fields().space_id().eq(space))
            .select(GroupRow::fields().id())
            .exec(&mut tx)
            .await?;
        let mut result = Vec::new();
        for row in OrganizationMemberRow::all()
            .filter(OrganizationMemberRow::fields().space_id().eq(space))
            .order_by(OrganizationMemberRow::fields().id().asc())
            .exec(&mut tx)
            .await?
        {
            let user = UserRow::filter_by_id(row.user_id).get(&mut tx).await?;
            let group_ids = GroupAccessRow::all()
                .filter(GroupAccessRow::fields().user_id().eq(user.id))
                .filter(GroupAccessRow::fields().group_id().in_list(groups.clone()))
                .select(GroupAccessRow::fields().group_id())
                .exec(&mut tx)
                .await?;
            result.push(OrganizationMember {
                id: row.id,
                user_id: user.id,
                email: user.email,
                enabled: user.enabled,
                role: OrganizationRole::parse(&row.role)?,
                group_ids,
                version: row.version,
            });
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn update_organization_member(
        &self,
        actor: i64,
        space: i64,
        id: i64,
        expected: u64,
        role: OrganizationRole,
        mut group_ids: Vec<i64>,
    ) -> StoreResult<()> {
        if role == OrganizationRole::Owner {
            return Err(StoreError::Validation("请使用转移所有权操作".into()));
        }
        group_ids.sort_unstable();
        group_ids.dedup();
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        owner(&mut tx, actor, space).await?;
        let mut row = member(&mut tx, space, id).await?;
        version(row.version, expected)?;
        if row.role == "owner" {
            return Err(StoreError::Validation(
                "所有者只能通过转移所有权变更".into(),
            ));
        }
        for group in &group_ids {
            let group = GroupRow::filter_by_id(*group)
                .first()
                .exec(&mut tx)
                .await?
                .ok_or(StoreError::NotFound)?;
            if group.space_id != Some(space) {
                return Err(StoreError::NotFound);
            }
        }
        clear_access(&mut tx, space, row.user_id).await?;
        if role == OrganizationRole::Member {
            for group in group_ids {
                GroupAccessRow::create()
                    .group_id(group)
                    .user_id(row.user_id)
                    .exec(&mut tx)
                    .await?;
            }
        }
        row.update().role(role.as_str()).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_organization_member(
        &self,
        actor: i64,
        space: i64,
        id: i64,
        expected: u64,
        revoke_keys: bool,
    ) -> StoreResult<()> {
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        owner(&mut tx, actor, space).await?;
        let row = member(&mut tx, space, id).await?;
        version(row.version, expected)?;
        if row.role == "owner" {
            return Err(StoreError::Validation("请先转移组织所有权".into()));
        }
        clear_access(&mut tx, space, row.user_id).await?;
        if revoke_keys {
            revoke_creator(&mut tx, space, row.user_id).await?;
        }
        OrganizationMemberRow::filter_by_id(id)
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn transfer_organization(
        &self,
        actor: i64,
        space: i64,
        target: i64,
        expected: u64,
    ) -> StoreResult<()> {
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        let (_, role, _) = Self::space_access(&mut tx, actor, space).await?;
        if role != OrganizationRole::Owner {
            return Err(StoreError::NotFound);
        }
        let mut target = member(&mut tx, space, target).await?;
        version(target.version, expected)?;
        active_user(&mut tx, target.user_id).await?;
        if target.role == "owner" {
            return Err(StoreError::Validation("该成员已经是所有者".into()));
        }
        let mut previous = OrganizationMemberRow::all()
            .filter(OrganizationMemberRow::fields().space_id().eq(space))
            .filter(OrganizationMemberRow::fields().user_id().eq(actor))
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        previous.update().role("admin").exec(&mut tx).await?;
        target.update().role("owner").exec(&mut tx).await?;
        clear_access(&mut tx, space, target.user_id).await?;
        tx.commit().await?;
        Ok(())
    }
}
