use super::*;
use crate::{
    model::{GroupAccessRow, GroupRow, OrganizationMemberRow, ResourceSpaceRow, UserRow},
    organizations::{OrganizationRole, ResourceSpace, SpaceKind},
};

impl ProviderStore {
    pub fn resource_space_id(&self) -> Option<i64> {
        self.space_id.or(Some(1))
    }
    pub fn actor_user_id(&self) -> Option<i64> {
        self.user_id
    }
    pub fn space_id(&self) -> Option<i64> {
        self.space_id
    }
    pub fn can_manage_resources(&self) -> bool {
        self.space_role.is_none_or(OrganizationRole::can_manage)
    }
    pub fn is_personal(&self) -> bool {
        self.personal_space
    }

    pub async fn for_user(&self, user_id: i64) -> StoreResult<Self> {
        let personal = self.ensure_personal_space(user_id).await?;
        Box::pin(self.for_space(user_id, personal)).await
    }

    pub async fn for_space(&self, user_id: i64, space_id: i64) -> StoreResult<Self> {
        let mut connection = self.connection().await?;
        let (space, role, admin) = Self::space_access(&mut connection, user_id, space_id).await?;
        drop(connection);
        let mut store = self.clone();
        store.user_id = Some(user_id);
        store.space_id = Some(space_id);
        store.space_role = Some(role);
        store.platform_admin = admin;
        store.personal_space = space.kind == "personal";
        let groups = store.list_groups().await?;
        if let Some(group) = groups.first() {
            store.group_id = group.id;
        } else if role.can_manage() && space.enabled {
            let _writer = self.chat_writes.lock().await;
            let mut connection = self.connection().await?;
            let mut tx = self.transaction(&mut connection, true).await?;
            store.bindings(&mut tx, true).await?;
            let existing = GroupRow::all()
                .filter(GroupRow::fields().space_id().eq(space_id))
                .first()
                .exec(&mut tx)
                .await?;
            store.group_id = match existing {
                Some(row) => row.id,
                None => {
                    GroupRow::create()
                        .space_id(space_id)
                        .name(if space.kind == "personal" {
                            "个人资源"
                        } else {
                            "default"
                        })
                        .enabled(true)
                        .updated_at(now()?)
                        .exec(&mut tx)
                        .await?
                        .id
                }
            };
            tx.commit().await?;
        } else {
            store.group_id = 0;
        }
        Ok(store)
    }

    pub(super) async fn ensure_personal_space(&self, user_id: i64) -> StoreResult<i64> {
        let mut connection = self.connection().await?;
        let user = UserRow::filter_by_id(user_id)
            .first()
            .exec(&mut connection)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !user.enabled {
            return Err(StoreError::NotFound);
        }
        if let Some(row) = ResourceSpaceRow::all()
            .filter(ResourceSpaceRow::fields().personal_user_id().eq(user_id))
            .first()
            .exec(&mut connection)
            .await?
        {
            return Ok(row.id);
        }
        let _writer = self.chat_writes.lock().await;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let row = ResourceSpaceRow::all()
            .filter(ResourceSpaceRow::fields().personal_user_id().eq(user_id))
            .first()
            .exec(&mut tx)
            .await?;
        let id = match row {
            Some(row) => row.id,
            None => {
                ResourceSpaceRow::create()
                    .kind("personal")
                    .name("个人空间")
                    .personal_user_id(user_id)
                    .enabled(true)
                    .created_at(now()?)
                    .exec(&mut tx)
                    .await?
                    .id
            }
        };
        tx.commit().await?;
        Ok(id)
    }

    pub(super) async fn space_access(
        executor: &mut dyn Executor,
        user_id: i64,
        space_id: i64,
    ) -> StoreResult<(ResourceSpaceRow, OrganizationRole, bool)> {
        let user = UserRow::filter_by_id(user_id)
            .first()
            .exec(&mut *executor)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !user.enabled {
            return Err(StoreError::NotFound);
        }
        let space = ResourceSpaceRow::filter_by_id(space_id)
            .first()
            .exec(&mut *executor)
            .await?
            .ok_or(StoreError::NotFound)?;
        let admin = user.role == "admin";
        let role = if space.kind == "personal" {
            if space.personal_user_id != Some(user_id) {
                return Err(StoreError::NotFound);
            }
            OrganizationRole::Owner
        } else {
            let member = OrganizationMemberRow::all()
                .filter(OrganizationMemberRow::fields().space_id().eq(space_id))
                .filter(OrganizationMemberRow::fields().user_id().eq(user_id))
                .first()
                .exec(&mut *executor)
                .await?;
            match member {
                Some(row) => OrganizationRole::parse(&row.role)?,
                None if admin => OrganizationRole::Admin,
                None => return Err(StoreError::NotFound),
            }
        };
        Ok((space, role, admin))
    }

    pub(super) async fn require_space(
        &self,
        executor: &mut dyn Executor,
        write: bool,
    ) -> StoreResult<()> {
        if let (Some(user_id), Some(space_id)) = (self.user_id, self.space_id) {
            let (space, role, _) = Self::space_access(executor, user_id, space_id).await?;
            if !space.enabled {
                return Err(StoreError::Validation("当前组织已停用".into()));
            }
            if write && !role.can_manage() {
                return Err(StoreError::Validation("当前角色没有资源管理权限".into()));
            }
        }
        Ok(())
    }

    pub(super) fn check_owner(&self, space_id: Option<i64>) -> StoreResult<()> {
        if self.space_id.is_some_and(|id| space_id != Some(id)) {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
    pub(super) async fn authorized_group_ids(
        &self,
        executor: &mut dyn Executor,
    ) -> StoreResult<Option<Vec<i64>>> {
        let (Some(user), Some(space_id)) = (self.user_id, self.space_id) else {
            return Ok(None);
        };
        let (space, role, _) = Self::space_access(executor, user, space_id).await?;
        if !space.enabled {
            return Ok(Some(Vec::new()));
        }
        if role.can_manage() {
            return Ok(None);
        }
        Ok(Some(
            GroupAccessRow::all()
                .filter(GroupAccessRow::fields().user_id().eq(user))
                .select(GroupAccessRow::fields().group_id())
                .exec(executor)
                .await?,
        ))
    }
    pub(super) async fn find_provider(
        &self,
        executor: &mut dyn Executor,
        id: i64,
    ) -> StoreResult<Provider> {
        Box::pin(self.require_space(executor, false)).await?;
        let provider = find(executor, id).await?;
        self.check_owner(provider.space_id)?;
        Ok(provider)
    }
    pub(super) async fn find_model(
        &self,
        executor: &mut dyn Executor,
        id: i64,
    ) -> StoreResult<ModelMapping> {
        let model = find_mapping(executor, id).await?;
        Box::pin(self.find_provider(executor, model.provider_id)).await?;
        Ok(model)
    }
    pub(super) async fn find_route(
        &self,
        executor: &mut dyn Executor,
        id: i64,
    ) -> StoreResult<ModelRouteRow> {
        Box::pin(self.require_space(executor, false)).await?;
        let row = ModelRouteRow::filter_by_id(id)
            .first()
            .exec(executor)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.check_owner(row.space_id)?;
        Ok(row)
    }
    pub(super) async fn provider_ids(&self, executor: &mut dyn Executor) -> StoreResult<Vec<i64>> {
        let mut query = Provider::all();
        if let Some(id) = self.space_id {
            query = query.filter(Provider::fields().space_id().eq(id));
        }
        Ok(query.select(Provider::fields().id()).exec(executor).await?)
    }
    pub async fn current_space(&self) -> StoreResult<Option<ResourceSpace>> {
        let (Some(user), Some(space)) = (self.user_id, self.space_id) else {
            return Ok(None);
        };
        let mut connection = self.connection().await?;
        let (row, role, _) = Self::space_access(&mut connection, user, space).await?;
        Ok(Some(ResourceSpace {
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
        }))
    }
}
#[cfg(test)]
mod tests;
