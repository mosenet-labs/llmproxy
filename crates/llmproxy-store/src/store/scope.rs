use super::*;
use crate::model::{GroupRow, UserRow};

impl ProviderStore {
    /// Console requests from ordinary users operate on their personal catalog.
    /// The unscoped store remains available to platform administration and the gateway.
    pub async fn for_user(&self, user_id: i64) -> StoreResult<Self> {
        let mut store = self.clone();
        store.user_id = Some(user_id);
        let mut connection = self.connection().await?;
        let user = UserRow::filter_by_id(user_id)
            .first()
            .exec(&mut connection)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !user.enabled {
            return Err(StoreError::NotFound);
        }
        if let Some(group) = GroupRow::all()
            .filter(GroupRow::fields().owner_user_id().eq(user_id))
            .order_by(GroupRow::fields().id().asc())
            .first()
            .exec(&mut connection)
            .await?
        {
            store.group_id = group.id;
            return Ok(store);
        }
        // SQLite must not synchronously wait for a writer on the same async worker.
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        // Serialize first-use creation, then recheck for concurrent requests.
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let user = UserRow::filter_by_id(user_id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !user.enabled {
            return Err(StoreError::NotFound);
        }
        let group = GroupRow::all()
            .filter(GroupRow::fields().owner_user_id().eq(user_id))
            .order_by(GroupRow::fields().id().asc())
            .first()
            .exec(&mut tx)
            .await?;
        store.group_id = match group {
            Some(group) => group.id,
            None => {
                GroupRow::create()
                    .owner_user_id(user_id)
                    .name("个人资源")
                    .enabled(true)
                    .updated_at(now()?)
                    .exec(&mut tx)
                    .await?
                    .id
            }
        };
        tx.commit().await?;
        Ok(store)
    }

    pub fn is_personal(&self) -> bool {
        self.user_id.is_some()
    }

    pub(super) fn check_owner(&self, owner: Option<i64>) -> StoreResult<()> {
        if self.user_id.is_some_and(|id| owner != Some(id)) {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub(super) async fn find_provider(
        &self,
        executor: &mut dyn Executor,
        id: i64,
    ) -> StoreResult<Provider> {
        let provider = find(executor, id).await?;
        self.check_owner(provider.owner_user_id)?;
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
        let route = ModelRouteRow::filter_by_id(id)
            .first()
            .exec(executor)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.check_owner(route.owner_user_id)?;
        Ok(route)
    }

    pub(super) async fn provider_ids(&self, executor: &mut dyn Executor) -> StoreResult<Vec<i64>> {
        let mut query = Provider::all();
        if let Some(id) = self.user_id {
            query = query.filter(Provider::fields().owner_user_id().eq(id));
        }
        Ok(query.select(Provider::fields().id()).exec(executor).await?)
    }
}

#[cfg(test)]
mod tests;
