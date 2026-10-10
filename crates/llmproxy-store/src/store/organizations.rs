use super::*;
use crate::{
    model::{
        GroupAccessRow, GroupRow, OrganizationInvitationRow, OrganizationMemberRow,
        ResourceSpaceRow, UserRow, VirtualKeyRow,
    },
    organizations::{
        OrganizationInvitation, OrganizationMember, OrganizationRole, ResourceSpace, SpaceKind,
    },
};

mod invitations;
mod members;

fn view(row: ResourceSpaceRow, role: OrganizationRole) -> ResourceSpace {
    ResourceSpace {
        id: row.id,
        name: row.name,
        kind: if row.kind == "personal" {
            SpaceKind::Personal
        } else {
            SpaceKind::Organization
        },
        enabled: row.enabled,
        role,
        version: row.version,
    }
}

async fn active_user(tx: &mut dyn Executor, user_id: i64) -> StoreResult<UserRow> {
    let user = UserRow::filter_by_id(user_id)
        .first()
        .exec(tx)
        .await?
        .ok_or(StoreError::NotFound)?;
    if !user.enabled {
        return Err(StoreError::NotFound);
    }
    Ok(user)
}

async fn owner(tx: &mut dyn Executor, actor: i64, space: i64) -> StoreResult<ResourceSpaceRow> {
    let (row, role, admin) = ProviderStore::space_access(tx, actor, space).await?;
    if row.kind != "organization" || (role != OrganizationRole::Owner && !admin) {
        return Err(StoreError::NotFound);
    }
    Ok(row)
}

fn version(actual: u64, expected: u64) -> StoreResult<()> {
    if actual != expected {
        return Err(StoreError::Conflict("记录已修改，请刷新后重试".into()));
    }
    Ok(())
}

impl ProviderStore {
    pub async fn list_spaces(&self, actor: i64) -> StoreResult<Vec<ResourceSpace>> {
        let personal = self.ensure_personal_space(actor).await?;
        let mut connection = self.connection().await?;
        let user = active_user(&mut connection, actor).await?;
        if user.role == "admin"
            && OrganizationMemberRow::all()
                .filter(OrganizationMemberRow::fields().space_id().eq(1_i64))
                .filter(OrganizationMemberRow::fields().role().eq("owner"))
                .first()
                .exec(&mut connection)
                .await?
                .is_none()
        {
            let _writer = self.chat_writes.lock().await;
            let mut tx = self.transaction(&mut connection, true).await?;
            locked_bindings(&mut tx, true, &self.backend).await?;
            // Fresh installations initialize the platform administrator after migrations.
            if user.role == "admin"
                && OrganizationMemberRow::all()
                    .filter(OrganizationMemberRow::fields().space_id().eq(1_i64))
                    .filter(OrganizationMemberRow::fields().role().eq("owner"))
                    .first()
                    .exec(&mut tx)
                    .await?
                    .is_none()
            {
                if let Some(mut member) = OrganizationMemberRow::all()
                    .filter(OrganizationMemberRow::fields().space_id().eq(1_i64))
                    .filter(OrganizationMemberRow::fields().user_id().eq(actor))
                    .first()
                    .exec(&mut tx)
                    .await?
                {
                    member.update().role("owner").exec(&mut tx).await?;
                } else {
                    OrganizationMemberRow::create()
                        .space_id(1_i64)
                        .user_id(actor)
                        .role("owner")
                        .exec(&mut tx)
                        .await?;
                }
            }
            tx.commit().await?;
        }
        let mut tx = self.transaction(&mut connection, false).await?;
        let memberships = OrganizationMemberRow::all()
            .filter(OrganizationMemberRow::fields().user_id().eq(actor))
            .exec(&mut tx)
            .await?;
        let mut result = vec![view(
            ResourceSpaceRow::filter_by_id(personal)
                .get(&mut tx)
                .await?,
            OrganizationRole::Owner,
        )];
        for row in ResourceSpaceRow::all()
            .filter(ResourceSpaceRow::fields().kind().eq("organization"))
            .order_by(ResourceSpaceRow::fields().id().asc())
            .exec(&mut tx)
            .await?
        {
            let role = memberships
                .iter()
                .find(|m| m.space_id == row.id)
                .map(|m| OrganizationRole::parse(&m.role))
                .transpose()?;
            if let Some(role) = role.or(if user.role == "admin" {
                Some(OrganizationRole::Admin)
            } else {
                None
            }) {
                result.push(view(row, role));
            }
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn create_organization(&self, actor: i64, name: &str) -> StoreResult<ResourceSpace> {
        let name = groups::label(name)?;
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        active_user(&mut tx, actor).await?;
        let row = ResourceSpaceRow::create()
            .kind("organization")
            .name(name)
            .enabled(true)
            .created_at(now()?)
            .exec(&mut tx)
            .await?;
        OrganizationMemberRow::create()
            .space_id(row.id)
            .user_id(actor)
            .role("owner")
            .exec(&mut tx)
            .await?;
        GroupRow::create()
            .space_id(row.id)
            .name("default")
            .enabled(true)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(view(row, OrganizationRole::Owner))
    }

    pub async fn update_organization(
        &self,
        actor: i64,
        space: i64,
        expected: u64,
        name: &str,
        enabled: bool,
    ) -> StoreResult<()> {
        let name = groups::label(name)?;
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        let mut row = owner(&mut tx, actor, space).await?;
        version(row.version, expected)?;
        row.update()
            .name(name)
            .enabled(enabled)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn revoke_keys_by_creator(
        &self,
        actor: i64,
        space: i64,
        user: i64,
    ) -> StoreResult<()> {
        let _writer = self.chat_writes.lock().await;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        locked_bindings(&mut tx, true, &self.backend).await?;
        let (row, role, _) = Self::space_access(&mut tx, actor, space).await?;
        if row.kind != "organization" || !role.can_manage() {
            return Err(StoreError::NotFound);
        }
        revoke_creator(&mut tx, space, user).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

async fn revoke_creator(tx: &mut dyn Executor, space: i64, user: i64) -> StoreResult<()> {
    let groups = GroupRow::all()
        .filter(GroupRow::fields().space_id().eq(space))
        .select(GroupRow::fields().id())
        .exec(&mut *tx)
        .await?;
    for mut key in VirtualKeyRow::all()
        .filter(VirtualKeyRow::fields().group_id().in_list(groups))
        .filter(VirtualKeyRow::fields().created_by_user_id().eq(user))
        .filter(VirtualKeyRow::fields().revoked().eq(false))
        .exec(&mut *tx)
        .await?
    {
        key.update()
            .enabled(false)
            .revoked(true)
            .exec(&mut *tx)
            .await?;
    }
    Ok(())
}
