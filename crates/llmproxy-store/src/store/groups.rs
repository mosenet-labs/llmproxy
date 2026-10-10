use super::*;
use crate::model::{GroupRow, ModelGroupMembership, RouteGroupMembership, VirtualKeyRow};
use crate::{CallIdentity, CreatedVirtualKey, GroupView, VirtualKeyInput, VirtualKeyView};
use aes_gcm::aead::Generate;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

impl ProviderStore {
    pub fn for_group(&self, group_id: i64) -> Self {
        let mut store = self.clone();
        store.group_id = group_id;
        store
    }

    pub fn group_id(&self) -> i64 {
        self.group_id
    }

    pub async fn list_groups(&self) -> StoreResult<Vec<GroupView>> {
        let mut connection = self.connection().await?;
        let mut query = GroupRow::all();
        if let Some(id) = self.user_id {
            query = query.filter(GroupRow::fields().owner_user_id().eq(id));
        }
        Ok(query
            .order_by(GroupRow::fields().id().asc())
            .exec(&mut connection)
            .await?
            .into_iter()
            .map(group_view)
            .collect())
    }

    pub async fn create_group(&self, name: &str) -> StoreResult<GroupView> {
        let name = label(name)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        check_unique_group_name(&mut tx, &name, None, self.user_id).await?;
        let view = group_view(
            GroupRow::create()
                .owner_user_id(self.user_id)
                .name(name)
                .enabled(true)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?,
        );
        tx.commit().await?;
        Ok(view)
    }

    pub async fn update_group(
        &self,
        id: i64,
        version: u64,
        name: &str,
        enabled: bool,
    ) -> StoreResult<GroupView> {
        let name = label(name)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut row = GroupRow::filter_by_id(id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.check_owner(row.owner_user_id)?;
        if row.version != version {
            return Err(StoreError::Conflict("组已被修改，请刷新".into()));
        }
        check_unique_group_name(&mut tx, &name, Some(id), row.owner_user_id).await?;
        row.update()
            .name(name)
            .enabled(enabled)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(group_view(row))
    }

    pub async fn group_enabled(&self) -> StoreResult<bool> {
        let mut connection = self.connection().await?;
        let group = GroupRow::filter_by_id(self.group_id)
            .first()
            .exec(&mut connection)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.check_owner(group.owner_user_id)?;
        Ok(group.enabled)
    }

    pub(super) async fn require_group(&self, executor: &mut dyn Executor) -> StoreResult<()> {
        let group = GroupRow::filter_by_id(self.group_id)
            .first()
            .exec(executor)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.check_owner(group.owner_user_id)
    }

    pub(super) fn owns(&self, group_id: i64) -> StoreResult<()> {
        if group_id != self.group_id {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub async fn list_virtual_keys(&self) -> StoreResult<Vec<VirtualKeyView>> {
        let mut connection = self.connection().await?;
        Box::pin(self.require_group(&mut connection)).await?;
        Ok(VirtualKeyRow::all()
            .filter(VirtualKeyRow::fields().group_id().eq(self.group_id))
            .order_by(VirtualKeyRow::fields().id().asc())
            .exec(&mut connection)
            .await?
            .into_iter()
            .map(key_view)
            .collect())
    }

    pub async fn create_virtual_key(
        &self,
        input: VirtualKeyInput,
    ) -> StoreResult<CreatedVirtualKey> {
        let name = label(&input.name)?;
        let current_time = now()?;
        if input
            .expires_at
            .is_some_and(|expiry| expiry <= current_time)
        {
            return Err(StoreError::Validation("过期时间须晚于当前时间".into()));
        }
        if input.all_routes && (!input.route_ids.is_empty() || !input.model_ids.is_empty()) {
            return Err(StoreError::Validation(
                "全部入口模式不能指定模型或路由".into(),
            ));
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        Box::pin(self.require_group(&mut tx)).await?;
        let mut ids = input.route_ids;
        ids.sort_unstable();
        ids.dedup();
        for id in &ids {
            let route = ModelRouteRow::filter_by_id(*id)
                .first()
                .exec(&mut tx)
                .await?
                .ok_or(StoreError::NotFound)?;
            self.require_route_member(&mut tx, route.id).await?;
        }
        let mut model_ids = input.model_ids;
        model_ids.sort_unstable();
        model_ids.dedup();
        for id in &model_ids {
            self.require_model_member(&mut tx, *id).await?;
        }
        let secret = format!("lp-vk-{}", URL_SAFE_NO_PAD.encode(<[u8; 32]>::generate()));
        let row = VirtualKeyRow::create()
            .group_id(self.group_id)
            .name(name)
            .digest(digest(&secret))
            .prefix(secret[..14].to_owned())
            .all_routes(input.all_routes)
            .route_ids(toasty::Json(ids))
            .model_ids(toasty::Json(model_ids))
            .enabled(true)
            .revoked(false)
            .expires_at(input.expires_at)
            .created_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(CreatedVirtualKey {
            view: key_view(row),
            secret,
        })
    }

    pub async fn set_virtual_key_enabled(
        &self,
        id: i64,
        version: u64,
        enabled: bool,
    ) -> StoreResult<()> {
        self.change_key(id, version, enabled, false).await
    }

    pub async fn revoke_virtual_key(&self, id: i64, version: u64) -> StoreResult<()> {
        self.change_key(id, version, false, true).await
    }

    async fn change_key(
        &self,
        id: i64,
        version: u64,
        enabled: bool,
        revoke: bool,
    ) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut row = VirtualKeyRow::filter_by_id(id)
            .first()
            .exec(&mut connection)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.owns(row.group_id)?;
        Box::pin(self.require_group(&mut connection)).await?;
        if row.version != version || row.revoked {
            return Err(StoreError::Conflict("Key 已修改或撤销".into()));
        }
        row.update()
            .enabled(enabled)
            .revoked(revoke)
            .exec(&mut connection)
            .await?;
        Ok(())
    }

    /// 每次请求读取 Key 与组状态，撤销无需等待路由快照刷新。
    pub async fn authenticate_virtual_key(
        &self,
        secret: &str,
    ) -> StoreResult<Option<CallIdentity>> {
        if !secret.starts_with("lp-vk-") || secret.len() != 49 {
            return Ok(None);
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let Some(row) = VirtualKeyRow::filter_by_digest(digest(secret))
            .first()
            .exec(&mut tx)
            .await?
        else {
            return Ok(None);
        };
        let current_time = now()?;
        if !row.enabled
            || row.revoked
            || row.expires_at.is_some_and(|expiry| expiry <= current_time)
        {
            return Ok(None);
        }
        let group = GroupRow::filter_by_id(row.group_id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::Internal)?;
        self.check_owner(group.owner_user_id)?;
        if !group.enabled {
            return Ok(None);
        }
        if let Some(user_id) = group.owner_user_id {
            let user = crate::model::UserRow::filter_by_id(user_id)
                .first()
                .exec(&mut tx)
                .await?;
            if !user.is_some_and(|user| user.enabled) {
                return Ok(None);
            }
        }
        let identity = CallIdentity {
            group_id: row.group_id,
            key_id: row.id,
            all_routes: row.all_routes,
            route_ids: row.route_ids.0,
            model_ids: row.model_ids.0,
        };
        tx.commit().await?;
        Ok(Some(identity))
    }
}

// Membership checks and resource writes share the store transaction and its write lock.
pub(super) async fn model_ids(executor: &mut dyn Executor, group_id: i64) -> StoreResult<Vec<i64>> {
    Ok(ModelGroupMembership::all()
        .filter(ModelGroupMembership::fields().group_id().eq(group_id))
        .select(ModelGroupMembership::fields().model_id())
        .exec(executor)
        .await?)
}

pub(super) async fn route_ids(executor: &mut dyn Executor, group_id: i64) -> StoreResult<Vec<i64>> {
    Ok(RouteGroupMembership::all()
        .filter(RouteGroupMembership::fields().group_id().eq(group_id))
        .select(RouteGroupMembership::fields().route_id())
        .exec(executor)
        .await?)
}

pub(super) async fn model_groups(
    executor: &mut dyn Executor,
    model_id: i64,
) -> StoreResult<Vec<i64>> {
    Ok(ModelGroupMembership::all()
        .filter(ModelGroupMembership::fields().model_id().eq(model_id))
        .select(ModelGroupMembership::fields().group_id())
        .exec(executor)
        .await?)
}

pub(super) async fn route_groups(
    executor: &mut dyn Executor,
    route_id: i64,
) -> StoreResult<Vec<i64>> {
    Ok(RouteGroupMembership::all()
        .filter(RouteGroupMembership::fields().route_id().eq(route_id))
        .select(RouteGroupMembership::fields().group_id())
        .exec(executor)
        .await?)
}

pub(super) async fn model_group_map(
    executor: &mut dyn Executor,
) -> StoreResult<HashMap<i64, Vec<i64>>> {
    let mut result: HashMap<i64, Vec<i64>> = HashMap::new();
    for (model, group) in ModelGroupMembership::all()
        .order_by(ModelGroupMembership::fields().group_id().asc())
        .select((
            ModelGroupMembership::fields().model_id(),
            ModelGroupMembership::fields().group_id(),
        ))
        .exec(executor)
        .await?
    {
        result.entry(model).or_default().push(group);
    }
    Ok(result)
}

pub(super) async fn route_group_map(
    executor: &mut dyn Executor,
) -> StoreResult<HashMap<i64, Vec<i64>>> {
    let mut result: HashMap<i64, Vec<i64>> = HashMap::new();
    for (route, group) in RouteGroupMembership::all()
        .order_by(RouteGroupMembership::fields().group_id().asc())
        .select((
            RouteGroupMembership::fields().route_id(),
            RouteGroupMembership::fields().group_id(),
        ))
        .exec(executor)
        .await?
    {
        result.entry(route).or_default().push(group);
    }
    Ok(result)
}

impl ProviderStore {
    pub(super) async fn require_model_member(
        &self,
        executor: &mut dyn Executor,
        id: i64,
    ) -> StoreResult<()> {
        Box::pin(self.require_group(executor)).await?;
        Box::pin(self.find_model(executor, id)).await?;
        if !model_groups(executor, id).await?.contains(&self.group_id) {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub(super) async fn require_route_member(
        &self,
        executor: &mut dyn Executor,
        id: i64,
    ) -> StoreResult<()> {
        Box::pin(self.require_group(executor)).await?;
        self.find_route(executor, id).await?;
        if !route_groups(executor, id).await?.contains(&self.group_id) {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub(super) async fn touch_group(executor: &mut dyn Executor, id: i64) -> StoreResult<()> {
        let mut group = GroupRow::filter_by_id(id)
            .first()
            .exec(executor)
            .await?
            .ok_or(StoreError::NotFound)?;
        group.update().updated_at(now()?).exec(executor).await?;
        Ok(())
    }

    pub(super) async fn join_model(
        &self,
        executor: &mut dyn Executor,
        model_id: i64,
    ) -> StoreResult<()> {
        ModelGroupMembership::create()
            .group_id(self.group_id)
            .model_id(model_id)
            .exec(executor)
            .await?;
        Ok(())
    }

    pub(super) async fn join_route(
        &self,
        executor: &mut dyn Executor,
        route_id: i64,
    ) -> StoreResult<()> {
        RouteGroupMembership::create()
            .group_id(self.group_id)
            .route_id(route_id)
            .exec(executor)
            .await?;
        Ok(())
    }

    /// Replace only group associations. Shared resource configuration and Key ownership stay intact.
    pub async fn set_group_resources(
        &self,
        version: u64,
        mut models: Vec<i64>,
        mut routes: Vec<i64>,
    ) -> StoreResult<()> {
        models.sort_unstable();
        models.dedup();
        routes.sort_unstable();
        routes.dedup();
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut group = GroupRow::filter_by_id(self.group_id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.check_owner(group.owner_user_id)?;
        if group.version != version {
            return Err(StoreError::Conflict("组已被修改，请刷新后重试".into()));
        }
        // Remove previous associations in this transaction before checking the complete replacement.
        ModelGroupMembership::all()
            .filter(ModelGroupMembership::fields().group_id().eq(self.group_id))
            .delete()
            .exec(&mut tx)
            .await?;
        RouteGroupMembership::all()
            .filter(RouteGroupMembership::fields().group_id().eq(self.group_id))
            .delete()
            .exec(&mut tx)
            .await?;
        for id in models {
            let model = Box::pin(self.find_model(&mut tx, id)).await?;
            let provider = find(&mut tx, model.provider_id).await?;
            if provider.owner_user_id != group.owner_user_id {
                return Err(StoreError::Validation("只能添加同一账户的模型".into()));
            }
            check_unique_alias(&mut tx, &model.alias, self.group_id, None).await?;
            models::check_route_name_available(&mut tx, &model.alias, self.group_id).await?;
            self.join_model(&mut tx, id).await?;
        }
        for id in routes {
            let route = self.find_route(&mut tx, id).await?;
            if route.owner_user_id != group.owner_user_id {
                return Err(StoreError::Validation("只能添加同一账户的模型路由".into()));
            }
            routes::check_unique_route_name(
                &mut tx,
                &route.name,
                self.group_id,
                routes::protocol_from_str(&route.protocol)?,
                None,
            )
            .await?;
            routes::check_model_alias_available(&mut tx, &route.name, self.group_id).await?;
            self.join_route(&mut tx, id).await?;
        }
        // Never reactivate old explicit grants when resources are removed and later re-added.
        let remaining = route_ids(&mut tx, self.group_id).await?;
        let remaining_models = model_ids(&mut tx, self.group_id).await?;
        for mut key in VirtualKeyRow::all()
            .filter(VirtualKeyRow::fields().group_id().eq(self.group_id))
            .exec(&mut tx)
            .await?
        {
            let grants: Vec<_> = key
                .route_ids
                .0
                .iter()
                .copied()
                .filter(|id| remaining.contains(id))
                .collect();
            let model_grants: Vec<_> = key
                .model_ids
                .0
                .iter()
                .copied()
                .filter(|id| remaining_models.contains(id))
                .collect();
            if grants != key.route_ids.0 || model_grants != key.model_ids.0 {
                key.update()
                    .route_ids(toasty::Json(grants))
                    .model_ids(toasty::Json(model_grants))
                    .exec(&mut tx)
                    .await?;
            }
        }
        group.update().updated_at(now()?).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

fn label(name: &str) -> StoreResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(StoreError::Validation("名称须为 1–80 个字符".into()));
    }
    Ok(name.to_owned())
}
fn digest(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}
fn group_view(row: GroupRow) -> GroupView {
    GroupView {
        id: row.id,
        name: row.name,
        enabled: row.enabled,
        version: row.version,
    }
}
fn key_view(row: VirtualKeyRow) -> VirtualKeyView {
    VirtualKeyView {
        id: row.id,
        group_id: row.group_id,
        name: row.name,
        prefix: row.prefix,
        all_routes: row.all_routes,
        route_ids: row.route_ids.0,
        model_ids: row.model_ids.0,
        enabled: row.enabled,
        revoked: row.revoked,
        expires_at: row.expires_at,
        version: row.version,
    }
}

async fn check_unique_group_name(
    executor: &mut dyn Executor,
    name: &str,
    own_id: Option<i64>,
    owner_user_id: Option<i64>,
) -> StoreResult<()> {
    if let Some(existing) = GroupRow::all()
        .filter(GroupRow::fields().name().eq(name))
        .filter(match owner_user_id {
            Some(id) => GroupRow::fields().owner_user_id().eq(id),
            None => GroupRow::fields().owner_user_id().is_none(),
        })
        .select(GroupRow::fields().id())
        .first()
        .exec(executor)
        .await?
        && Some(existing) != own_id
    {
        return Err(StoreError::Conflict(
            "已存在同名资源组，请使用其他名称".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelRouteTargetInput, ProviderPaths};

    async fn resource_store(label: &str) -> (ProviderStore, std::path::PathBuf, i64) {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-{label}-{}-{}",
            std::process::id(),
            now().unwrap()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let store = ProviderStore::connect(
            &format!("sqlite:{}", directory.join("db.sqlite").display()),
            &base64::engine::general_purpose::STANDARD.encode([7; 32]),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        let provider = store
            .create(ProviderInput {
                name: "shared".into(),
                paths: ProviderPaths::single(Protocol::OpenAiChat),
                host: "example.com".into(),
                port: 443,
                tls: true,
                api_key: "test".into(),
                enabled: true,
                anthropic_version: None,
                connect_timeout_ms: 1000,
                read_timeout_ms: 1000,
                write_timeout_ms: 1000,
                models_path: "/models".into(),
                models_protocol: Protocol::OpenAiChat,
                messages_auth: crate::MessagesAuth::ApiKey,
            })
            .await
            .unwrap();
        (store, directory, provider.id)
    }

    fn resource_model(provider_id: i64, alias: &str) -> ModelMappingInput {
        ModelMappingInput {
            thinking: Default::default(),
            alias: alias.into(),
            provider_id,
            upstream_model_id: "upstream".into(),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
        }
    }

    fn resource_route(model_id: i64, name: &str) -> ModelRouteInput {
        ModelRouteInput {
            name: name.into(),
            protocol: Protocol::OpenAiChat,
            provider_protocol: Protocol::OpenAiChat,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id,
                enabled: true,
            }],
        }
    }

    async fn resource_version(store: &ProviderStore) -> u64 {
        store
            .list_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.id == store.group_id())
            .unwrap()
            .version
    }

    #[tokio::test]
    async fn model_grants_are_scoped_and_removal_does_not_restore_them() {
        let (store, directory, provider) = resource_store("model-key-grants").await;
        let model = store
            .create_model(resource_model(provider, "direct"))
            .await
            .unwrap();
        let route = store
            .create_route(resource_route(model.id, "routed"))
            .await
            .unwrap();
        store
            .set_group_resources(
                resource_version(&store).await,
                vec![model.id],
                vec![route.id],
            )
            .await
            .unwrap();
        let input = VirtualKeyInput {
            name: "models".into(),
            all_routes: false,
            model_ids: vec![model.id, model.id],
            route_ids: vec![],
            expires_at: None,
        };
        let other = store.for_group(store.create_group("other").await.unwrap().id);
        assert!(matches!(
            other.create_virtual_key(input.clone()).await,
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store
                .create_virtual_key(VirtualKeyInput {
                    all_routes: true,
                    ..input.clone()
                })
                .await,
            Err(StoreError::Validation(_))
        ));
        let key = store.create_virtual_key(input.clone()).await.unwrap();
        let mixed = store
            .create_virtual_key(VirtualKeyInput {
                name: "mixed".into(),
                route_ids: vec![route.id],
                ..input
            })
            .await
            .unwrap();
        assert_eq!(key.view.model_ids, vec![model.id]);
        assert!(key.view.route_ids.is_empty());
        assert_eq!(
            store
                .authenticate_virtual_key(&key.secret)
                .await
                .unwrap()
                .unwrap()
                .model_ids,
            vec![model.id]
        );
        store
            .set_group_resources(resource_version(&store).await, vec![], vec![route.id])
            .await
            .unwrap();
        for key in store.list_virtual_keys().await.unwrap() {
            assert!(key.model_ids.is_empty());
            assert!(key.version > mixed.view.version);
            if key.id == mixed.view.id {
                assert_eq!(key.route_ids, vec![route.id]);
            }
        }
        store
            .set_group_resources(
                resource_version(&store).await,
                vec![model.id],
                vec![route.id],
            )
            .await
            .unwrap();
        assert!(
            store
                .authenticate_virtual_key(&key.secret)
                .await
                .unwrap()
                .unwrap()
                .model_ids
                .is_empty()
        );
        drop((store, other));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn resources_join_multiple_groups_independently_and_removed_grants_stay_removed() {
        let (store, directory, provider) = resource_store("resource-memberships").await;
        let group = store.create_group("apps").await.unwrap();
        let apps = store.for_group(group.id);
        let model = store
            .create_model(resource_model(provider, "direct"))
            .await
            .unwrap();
        // A route in another group can reference this model without exposing its alias there.
        let route = apps
            .create_route(resource_route(model.id, "public"))
            .await
            .unwrap();
        assert!(model.group_ids.is_empty());
        assert!(route.group_ids.is_empty());
        assert!(store.list_models().await.unwrap().is_empty());
        assert!(store.list_routes().await.unwrap().is_empty());
        assert!(apps.load_model_routes().await.unwrap().is_empty());
        assert!(matches!(
            apps.load_model_route(model.id, Protocol::OpenAiChat).await,
            Err(StoreError::NotFound)
        ));
        assert_eq!(apps.list_all_models().await.unwrap().len(), 1);
        assert_eq!(apps.list_all_routes().await.unwrap().len(), 1);
        apps.set_group_resources(resource_version(&apps).await, vec![], vec![route.id])
            .await
            .unwrap();
        assert!(apps.list_models().await.unwrap().is_empty());
        assert_eq!(
            apps.load_model_routes()
                .await
                .unwrap()
                .iter()
                .map(|r| r.alias.as_str())
                .collect::<Vec<_>>(),
            vec!["public"]
        );
        apps.set_group_resources(
            resource_version(&apps).await,
            vec![model.id],
            vec![route.id],
        )
        .await
        .unwrap();
        assert_eq!(
            apps.get_model(model.id).await.unwrap().group_ids,
            vec![group.id]
        );
        store
            .set_group_resources(
                resource_version(&store).await,
                vec![model.id],
                vec![route.id],
            )
            .await
            .unwrap();
        let key = apps
            .create_virtual_key(VirtualKeyInput {
                name: "app".into(),
                all_routes: false,
                model_ids: vec![],
                route_ids: vec![route.id],
                expires_at: None,
            })
            .await
            .unwrap();
        assert_eq!(key.view.group_id, group.id);
        assert!(store.list_virtual_keys().await.unwrap().is_empty());
        assert_eq!(apps.list_virtual_keys().await.unwrap().len(), 1);
        let previous = resource_version(&apps).await;
        apps.set_group_resources(previous, vec![], vec![])
            .await
            .unwrap();
        assert!(apps.list_models().await.unwrap().is_empty());
        assert!(apps.list_routes().await.unwrap().is_empty());
        assert!(apps.load_model_routes().await.unwrap().is_empty());
        assert_eq!(store.list_all_models().await.unwrap().len(), 1);
        assert_eq!(store.list_routes().await.unwrap().len(), 1);
        assert!(matches!(
            apps.set_group_resources(previous, vec![model.id], vec![route.id])
                .await,
            Err(StoreError::Conflict(_))
        ));
        apps.set_group_resources(resource_version(&apps).await, vec![], vec![route.id])
            .await
            .unwrap();
        let identity = store
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(identity.group_id, group.id);
        assert!(identity.route_ids.is_empty());
        assert_eq!(
            apps.list_virtual_keys().await.unwrap()[0].route_ids,
            Vec::<i64>::new()
        );
        drop((store, apps));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn resource_membership_conflicts_roll_back_and_shared_edits_check_every_group() {
        let (store, directory, provider) = resource_store("resource-collisions").await;
        let group = store.create_group("other").await.unwrap();
        let other = store.for_group(group.id);
        let first = store
            .create_model(resource_model(provider, "same"))
            .await
            .unwrap();
        let second = other
            .create_model(resource_model(provider, "same"))
            .await
            .unwrap();
        let route = other
            .create_route(resource_route(second.id, "route"))
            .await
            .unwrap();
        let first = store
            .update_model(first.id, first.version, resource_model(provider, "same"))
            .await
            .unwrap();
        let route = store
            .update_route(route.id, route.version, resource_route(second.id, "route"))
            .await
            .unwrap();
        assert!(first.group_ids.is_empty());
        assert!(route.group_ids.is_empty());
        store
            .set_group_resources(resource_version(&store).await, vec![first.id], vec![])
            .await
            .unwrap();
        other
            .set_group_resources(
                resource_version(&other).await,
                vec![second.id],
                vec![route.id],
            )
            .await
            .unwrap();
        let version = resource_version(&other).await;
        assert!(matches!(
            other
                .set_group_resources(version, vec![first.id, second.id], vec![])
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(other.list_models().await.unwrap()[0].id, second.id);
        assert_eq!(other.list_routes().await.unwrap()[0].id, route.id);
        assert_eq!(resource_version(&other).await, version);
        other
            .set_group_resources(version, vec![first.id], vec![route.id])
            .await
            .unwrap();
        let collision = other
            .create_model(resource_model(provider, "reserved"))
            .await
            .unwrap();
        other
            .set_group_resources(
                resource_version(&other).await,
                vec![first.id, collision.id],
                vec![route.id],
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .update_model(
                    first.id,
                    first.version,
                    resource_model(provider, "reserved")
                )
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(store.get_model(first.id).await.unwrap().alias, "same");
        // Catalog creation does not change group membership or invalidate its version.
        let before_create = resource_version(&other).await;
        let unassigned = other
            .create_model(resource_model(provider, "new"))
            .await
            .unwrap();
        store
            .delete_model(unassigned.id, unassigned.version)
            .await
            .unwrap();
        assert_eq!(resource_version(&other).await, before_create);
        other
            .set_group_resources(before_create, vec![collision.id], vec![])
            .await
            .unwrap();
        drop((store, other));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn shared_provider_upgrade_preserves_duplicate_names_credentials_and_references() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-shared-providers-{}-{}",
            std::process::id(),
            now().unwrap()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("db.sqlite").display());
        let key = base64::engine::general_purpose::STANDARD.encode([7; 32]);
        let store = ProviderStore::connect(&url, &key).await.unwrap();
        let db = Db::builder().max_pool_size(1).connect(&url).await.unwrap();
        let migrations: &'static [_] = SQLITE_MIGRATIONS.migrations();
        toasty::migration::MigrationSet::new(&migrations[..57])
            .apply(&db)
            .await
            .unwrap();
        let mut connection = db.connection().await.unwrap();
        toasty::sql::statement("INSERT INTO groups (id, name, updated_at) VALUES (2, 'second', 0)")
            .exec(&mut connection)
            .await
            .unwrap();
        for (id, secret) in [(1, "first-credential"), (2, "second-credential")] {
            let encrypted = store.cipher.encrypt(secret).unwrap();
            toasty::sql::statement(format!("INSERT INTO providers (id, group_id, name, host, port, tls, encrypted_key, enabled, connect_timeout_ms, read_timeout_ms, write_timeout_ms, updated_at, openai_chat_path) VALUES ({id}, {id}, 'same', 'example.com', 443, 1, '{encrypted}', 1, 1000, 1000, 1000, 0, '/v1/chat/completions')"))
                .exec(&mut connection).await.unwrap();
            toasty::sql::statement(format!("INSERT INTO model_mappings (id, group_id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, updated_at) VALUES ({id}, {id}, 'same-model', {id}, 'real', 1, 0, 0, 0, 0)"))
                .exec(&mut connection).await.unwrap();
        }
        toasty::sql::statement("INSERT INTO model_routes (id, group_id, name, protocol, provider_protocol, updated_at) VALUES (1, 2, 'route', 'openai_chat', 'openai_chat', 0)")
            .exec(&mut connection).await.unwrap();
        toasty::sql::statement("INSERT INTO model_route_targets (route_id, model_id, position, enabled) VALUES (1, 2, 0, 1)")
            .exec(&mut connection).await.unwrap();
        toasty::sql::statement("INSERT INTO virtual_keys (group_id, name, digest, prefix, all_routes, route_ids, created_at) VALUES (2, 'legacy', 'legacy-digest', 'legacy-prefix', FALSE, '[1]', 0)")
            .exec(&mut connection).await.unwrap();
        // Deleted IDs must not be reused after rebuilding the resource tables.
        toasty::sql::statement("INSERT INTO model_mappings (id, group_id, alias, provider_id, upstream_model_id, openai_chat, openai_responses, anthropic_messages, gemini, updated_at) VALUES (100, 1, 'deleted-model', 1, 'old', 1, 0, 0, 0, 0)").exec(&mut connection).await.unwrap();
        toasty::sql::statement("DELETE FROM model_mappings WHERE id = 100")
            .exec(&mut connection)
            .await
            .unwrap();
        toasty::sql::statement("INSERT INTO model_routes (id, group_id, name, protocol, provider_protocol, updated_at) VALUES (100, 1, 'deleted-route', 'openai_chat', 'openai_chat', 0)").exec(&mut connection).await.unwrap();
        toasty::sql::statement("DELETE FROM model_routes WHERE id = 100")
            .exec(&mut connection)
            .await
            .unwrap();
        drop(connection);
        drop(db);
        store.migrate().await.unwrap();
        store.migrate().await.unwrap();
        let second = store.for_group(2);
        let legacy = second.list_virtual_keys().await.unwrap().remove(0);
        assert!(!legacy.all_routes);
        assert_eq!(legacy.route_ids, vec![1]);
        assert!(legacy.model_ids.is_empty());
        assert_eq!(store.list().await.unwrap().len(), 2);
        assert_eq!(second.list().await.unwrap().len(), 2);
        assert_eq!(
            store.probe_target(1).await.unwrap().secret,
            "first-credential"
        );
        assert_eq!(
            second.probe_target(2).await.unwrap().secret,
            "second-credential"
        );
        assert_eq!(store.get_model(1).await.unwrap().provider_id, 1);
        let model = second.get_model(2).await.unwrap();
        assert_eq!(model.provider_id, 2);
        assert!(model.group_ids.is_empty());
        assert!(second.list_routes().await.unwrap().is_empty());
        assert_eq!(
            second.list_all_routes().await.unwrap()[0].targets[0]
                .model
                .id,
            2
        );
        let provider = second.get(2).await.unwrap();
        second
            .set_enabled(2, provider.version, false)
            .await
            .unwrap();
        assert!(!store.get(2).await.unwrap().enabled);
        assert!(!second.get_model(2).await.unwrap().provider_enabled);
        let disabled = store.get(2).await.unwrap();
        assert!(matches!(
            store.delete(2, disabled.version).await,
            Err(StoreError::Conflict(_))
        ));
        let next_model = store
            .create_model(resource_model(1, "new-model"))
            .await
            .unwrap();
        assert!(next_model.id > 100);
        let next_route = store
            .create_route(resource_route(next_model.id, "new-route"))
            .await
            .unwrap();
        assert!(next_route.id > 100);
        drop(second);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn group_names_reject_duplicates_without_changing_existing_groups() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-group-names-{}-{}",
            std::process::id(),
            now().unwrap()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let store = ProviderStore::connect(
            &format!("sqlite:{}", directory.join("db.sqlite").display()),
            &base64::engine::general_purpose::STANDARD.encode([7; 32]),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        let group = store.create_group("test").await.unwrap();
        let duplicate = StoreError::Conflict("已存在同名资源组，请使用其他名称".into());
        for name in ["test", " test ", "default"] {
            assert_eq!(store.create_group(name).await.unwrap_err(), duplicate);
        }
        assert_eq!(
            store
                .update_group(group.id, group.version, " default ", false)
                .await
                .unwrap_err(),
            duplicate
        );
        let unchanged = store
            .list_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.id == group.id)
            .unwrap();
        assert_eq!(unchanged.name, group.name);
        assert_eq!(unchanged.version, group.version);
        assert_eq!(unchanged.enabled, group.enabled);
        let updated = store
            .update_group(group.id, group.version, " test ", false)
            .await
            .unwrap();
        assert_eq!(updated.name, "test");
        assert!(!updated.enabled);
        assert_eq!(store.list_groups().await.unwrap().len(), 2);
        let first_store = store.clone();
        let second_store = store.clone();
        let first = tokio::spawn(async move { first_store.create_group("concurrent").await });
        let second = tokio::spawn(async move { second_store.create_group("concurrent").await });
        let (first, second) = tokio::join!(first, second);
        match (first.unwrap(), second.unwrap()) {
            (Ok(_), Err(error)) | (Err(error), Ok(_)) => assert_eq!(error, duplicate),
            other => panic!("expected one successful create and one duplicate error: {other:?}"),
        }
        assert_eq!(store.list_groups().await.unwrap().len(), 3);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn groups_isolate_resources_and_key_lifecycle() {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-groups-{}-{}",
            std::process::id(),
            now().unwrap()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let store = ProviderStore::connect(
            &format!("sqlite:{}", directory.join("db.sqlite").display()),
            &base64::engine::general_purpose::STANDARD.encode([7; 32]),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        let group = store.create_group("second").await.unwrap();
        let second = store.for_group(group.id);
        let input = ProviderInput {
            name: "same".into(),
            paths: ProviderPaths::single(Protocol::OpenAiChat),
            host: "example.com".into(),
            port: 443,
            tls: true,
            api_key: "upstream".into(),
            enabled: true,
            models_path: "/models".into(),
            models_protocol: Protocol::OpenAiChat,
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
        };
        let a = store.create(input.clone()).await.unwrap();
        assert!(matches!(
            second.create(input).await,
            Err(StoreError::Conflict(_))
        ));
        let b = second.get(a.id).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 1);
        assert_eq!(second.list().await.unwrap().len(), 1);
        assert_eq!(a.id, b.id);
        second.probe_target(a.id).await.unwrap();
        let mapping = |provider_id| ModelMappingInput {
            thinking: Default::default(),
            alias: "same-model".into(),
            provider_id,
            upstream_model_id: "real".into(),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
        };
        let ma = store.create_model(mapping(a.id)).await.unwrap();
        let mb = second.create_model(mapping(b.id)).await.unwrap();
        assert!(ma.group_ids.is_empty());
        assert!(mb.group_ids.is_empty());
        assert_eq!(store.provider_health_checks(a.id).await.unwrap().len(), 2);
        second.set_enabled(a.id, a.version, false).await.unwrap();
        assert!(!store.get_model(ma.id).await.unwrap().provider_enabled);
        assert!(!second.get_model(mb.id).await.unwrap().provider_enabled);
        let disabled_provider = store.get(a.id).await.unwrap();
        store
            .set_enabled(a.id, disabled_provider.version, true)
            .await
            .unwrap();
        assert_eq!(store.get_model(mb.id).await.unwrap().id, mb.id);
        assert!(matches!(
            store.load_model_route(mb.id, Protocol::OpenAiChat).await,
            Err(StoreError::NotFound)
        ));
        let route = |model_id| ModelRouteInput {
            name: "same-route".into(),
            protocol: Protocol::OpenAiChat,
            provider_protocol: Protocol::OpenAiChat,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id,
                enabled: true,
            }],
        };
        let ra = store.create_route(route(ma.id)).await.unwrap();
        let rb = second.create_route(route(mb.id)).await.unwrap();
        store
            .set_group_resources(resource_version(&store).await, vec![ma.id], vec![ra.id])
            .await
            .unwrap();
        second
            .set_group_resources(resource_version(&second).await, vec![mb.id], vec![rb.id])
            .await
            .unwrap();
        let updated = store
            .update_route(ra.id, ra.version, route(mb.id))
            .await
            .unwrap();
        assert_eq!(updated.targets[0].model.id, mb.id);
        assert_eq!(
            store
                .load_model_routes()
                .await
                .unwrap()
                .iter()
                .find(|r| r.alias == "same-route")
                .unwrap()
                .model_id,
            Some(mb.id)
        );
        assert_eq!(store.get_model(mb.id).await.unwrap().id, mb.id);
        assert!(matches!(
            store.load_model_route(mb.id, Protocol::OpenAiChat).await,
            Err(StoreError::NotFound)
        ));
        let key_input = |route_ids| VirtualKeyInput {
            name: "client".into(),
            all_routes: false,
            model_ids: vec![],
            route_ids,
            expires_at: None,
        };
        assert!(matches!(
            store.create_virtual_key(key_input(vec![rb.id])).await,
            Err(StoreError::NotFound)
        ));
        let key = store
            .create_virtual_key(key_input(vec![ra.id]))
            .await
            .unwrap();
        let identity = second
            .authenticate_virtual_key(&key.secret)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(identity.group_id, 1);
        assert_eq!(identity.route_ids, vec![ra.id]);
        assert!(!identity.all_routes);
        assert!(
            store
                .authenticate_virtual_key("upstream")
                .await
                .unwrap()
                .is_none()
        );
        let mut connection = store.connection().await.unwrap();
        let persisted = VirtualKeyRow::filter_by_id(key.view.id)
            .first()
            .exec(&mut connection)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(persisted.digest, key.secret);
        assert_eq!(persisted.digest.len(), 64);
        drop(connection);
        store
            .set_virtual_key_enabled(key.view.id, key.view.version, false)
            .await
            .unwrap();
        assert!(
            store
                .authenticate_virtual_key(&key.secret)
                .await
                .unwrap()
                .is_none()
        );
        let view = store.list_virtual_keys().await.unwrap().remove(0);
        store
            .set_virtual_key_enabled(view.id, view.version, true)
            .await
            .unwrap();
        assert!(
            store
                .authenticate_virtual_key(&key.secret)
                .await
                .unwrap()
                .is_some()
        );
        let default_group = store.list_groups().await.unwrap().remove(0);
        let disabled = store
            .update_group(default_group.id, default_group.version, "default", false)
            .await
            .unwrap();
        assert!(
            store
                .authenticate_virtual_key(&key.secret)
                .await
                .unwrap()
                .is_none()
        );
        store
            .update_group(disabled.id, disabled.version, "default", true)
            .await
            .unwrap();
        let view = store.list_virtual_keys().await.unwrap().remove(0);
        store
            .revoke_virtual_key(view.id, view.version)
            .await
            .unwrap();
        assert!(
            store
                .authenticate_virtual_key(&key.secret)
                .await
                .unwrap()
                .is_none()
        );
        let view = store.list_virtual_keys().await.unwrap().remove(0);
        assert!(
            store
                .set_virtual_key_enabled(view.id, view.version, true)
                .await
                .is_err()
        );
        let mut expired = key_input(vec![]);
        expired.expires_at = Some(now().unwrap() - 1);
        assert!(store.create_virtual_key(expired).await.is_err());
        let mut expiring = key_input(vec![]);
        expiring.expires_at = Some(now().unwrap() + 100);
        let expiring = store.create_virtual_key(expiring).await.unwrap();
        let mut connection = store.connection().await.unwrap();
        let mut row = VirtualKeyRow::filter_by_id(expiring.view.id)
            .first()
            .exec(&mut connection)
            .await
            .unwrap()
            .unwrap();
        row.update()
            .expires_at(Some(now().unwrap() - 1))
            .exec(&mut connection)
            .await
            .unwrap();
        drop(connection);
        assert!(
            store
                .authenticate_virtual_key(&expiring.secret)
                .await
                .unwrap()
                .is_none()
        );
        let empty = store.create_virtual_key(key_input(vec![])).await.unwrap();
        let identity = store
            .authenticate_virtual_key(&empty.secret)
            .await
            .unwrap()
            .unwrap();
        assert!(!identity.all_routes && identity.route_ids.is_empty());
        let room = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let selection = llmproxy_core::conversation::Selection {
            model_id: ma.id.to_string(),
            protocol: Protocol::OpenAiChat,
        };
        store
            .create_chat_conversation(room, &selection)
            .await
            .unwrap();
        assert!(second.get_chat_conversation(room).await.is_err());
        drop((store, second));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
