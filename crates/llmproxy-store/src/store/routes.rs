use super::*;

impl ProviderStore {
    pub async fn list_routes(&self) -> StoreResult<Vec<ModelRouteView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let rows = ModelRouteRow::all()
            .order_by(ModelRouteRow::fields().id().asc())
            .exec(&mut tx)
            .await?;
        let mut routes = Vec::with_capacity(rows.len());
        for row in rows {
            routes.push(route_view(&mut tx, &row).await?);
        }
        tx.commit().await?;
        Ok(routes)
    }

    pub async fn create_route(&self, input: ModelRouteInput) -> StoreResult<ModelRouteView> {
        let input = validate_route(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        check_unique_route_name(&mut tx, &input.name, input.protocol, None).await?;
        check_model_alias_available(&mut tx, &input.name).await?;
        check_route_targets(&mut tx, input.protocol, &input.targets).await?;
        let row = ModelRouteRow::create()
            .name(input.name)
            .protocol(input.protocol.as_str().to_owned())
            .enabled(input.enabled)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        insert_route_targets(&mut tx, row.id, &input.targets).await?;
        let view = route_view(&mut tx, &row).await?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn update_route(
        &self,
        id: i64,
        version: u64,
        input: ModelRouteInput,
    ) -> StoreResult<ModelRouteView> {
        let input = validate_route(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut row = find_route(&mut tx, id).await?;
        check_route_version(&row, version)?;
        check_unique_route_name(&mut tx, &input.name, input.protocol, Some(id)).await?;
        if input.name != row.name {
            check_model_alias_available(&mut tx, &input.name).await?;
        }
        check_route_targets(&mut tx, input.protocol, &input.targets).await?;
        for target in ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().route_id().eq(id))
            .exec(&mut tx)
            .await?
        {
            target.delete().exec(&mut tx).await?;
        }
        row.update()
            .name(input.name)
            .protocol(input.protocol.as_str().to_owned())
            .enabled(input.enabled)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        insert_route_targets(&mut tx, id, &input.targets).await?;
        let view = route_view(&mut tx, &row).await?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn delete_route(&self, id: i64, version: u64) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let row = find_route(&mut tx, id).await?;
        check_route_version(&row, version)?;
        for target in ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().route_id().eq(id))
            .exec(&mut tx)
            .await?
        {
            target.delete().exec(&mut tx).await?;
        }
        row.delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn find_route(executor: &mut dyn Executor, id: i64) -> StoreResult<ModelRouteRow> {
    ModelRouteRow::filter_by_id(id)
        .first()
        .exec(executor)
        .await?
        .ok_or(StoreError::NotFound)
}

async fn route_view(tx: &mut Transaction<'_>, row: &ModelRouteRow) -> StoreResult<ModelRouteView> {
    let targets = ModelRouteTargetRow::all()
        .filter(ModelRouteTargetRow::fields().route_id().eq(row.id))
        .order_by(ModelRouteTargetRow::fields().position().asc())
        .exec(&mut *tx)
        .await?;
    let mut views = Vec::with_capacity(targets.len());
    for target in targets {
        let mapping = find_mapping(&mut *tx, target.model_id).await?;
        let provider = find(&mut *tx, mapping.provider_id).await?;
        views.push(ModelRouteTargetView {
            model: mapping_view(&mapping, &provider),
            enabled: target.enabled,
        });
    }
    Ok(ModelRouteView {
        id: row.id,
        name: row.name.clone(),
        protocol: protocol_from_str(&row.protocol)?,
        enabled: row.enabled,
        targets: views,
        version: row.version,
    })
}

fn validate_route(mut input: ModelRouteInput) -> StoreResult<ModelRouteInput> {
    input.name = input.name.trim().to_owned();
    if input.name.is_empty() || input.name.len() > 200 || input.name.chars().any(char::is_control) {
        return Err(StoreError::Validation("路由名称须为 1–200 个字符".into()));
    }
    if input.targets.is_empty() {
        return Err(StoreError::Validation("请至少添加一个候选模型".into()));
    }
    let mut ids = HashSet::new();
    if input
        .targets
        .iter()
        .any(|target| !ids.insert(target.model_id))
    {
        return Err(StoreError::Validation("候选模型不能重复".into()));
    }
    Ok(input)
}

async fn check_route_targets(
    tx: &mut Transaction<'_>,
    protocol: Protocol,
    targets: &[crate::ModelRouteTargetInput],
) -> StoreResult<()> {
    for target in targets {
        let mapping = find_mapping(&mut *tx, target.model_id).await?;
        if !mapping.protocols().contains(&protocol) {
            return Err(StoreError::Validation("候选模型不支持路由协议".into()));
        }
    }
    Ok(())
}

pub(super) async fn check_unique_route_name(
    tx: &mut Transaction<'_>,
    name: &str,
    protocol: Protocol,
    own_id: Option<i64>,
) -> StoreResult<()> {
    if ModelRouteRow::all()
        .filter(ModelRouteRow::fields().name().eq(name))
        .exec(tx)
        .await?
        .into_iter()
        .any(|existing| existing.protocol == protocol.as_str() && Some(existing.id) != own_id)
    {
        return Err(StoreError::Conflict("模型路由名称已存在".into()));
    }
    Ok(())
}

pub(super) fn protocol_from_str(value: &str) -> StoreResult<Protocol> {
    match value {
        "openai_chat" => Ok(Protocol::OpenAiChat),
        "openai_responses" => Ok(Protocol::OpenAiResponses),
        "anthropic_messages" => Ok(Protocol::AnthropicMessages),
        "gemini" => Ok(Protocol::Gemini),
        _ => Err(StoreError::Internal),
    }
}

async fn check_model_alias_available(tx: &mut Transaction<'_>, name: &str) -> StoreResult<()> {
    if ModelMapping::filter_by_alias(name)
        .first()
        .exec(tx)
        .await?
        .is_some()
    {
        return Err(StoreError::Conflict("路由名称与已有模型标识重复".into()));
    }
    Ok(())
}

fn check_route_version(row: &ModelRouteRow, version: u64) -> StoreResult<()> {
    if row.version != version {
        return Err(StoreError::Conflict(
            "模型路由已被修改，请刷新后重试".into(),
        ));
    }
    Ok(())
}

async fn insert_route_targets(
    tx: &mut Transaction<'_>,
    route_id: i64,
    targets: &[crate::ModelRouteTargetInput],
) -> StoreResult<()> {
    for (position, target) in targets.iter().enumerate() {
        ModelRouteTargetRow::create()
            .route_id(route_id)
            .model_id(target.model_id)
            .position(position as i64)
            .enabled(target.enabled)
            .exec(&mut *tx)
            .await?;
    }
    Ok(())
}
