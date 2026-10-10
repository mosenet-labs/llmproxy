use super::*;

impl ProviderStore {
    /// Routes explicitly selected for this group (Chat and gateway scope).
    pub async fn list_routes(&self) -> StoreResult<Vec<ModelRouteView>> {
        Box::pin(self.routes_in_group(Some(self.group_id))).await
    }

    /// System route catalog, including routes not selected for any group.
    pub async fn list_all_routes(&self) -> StoreResult<Vec<ModelRouteView>> {
        Box::pin(self.routes_in_group(None)).await
    }

    async fn routes_in_group(&self, group: Option<i64>) -> StoreResult<Vec<ModelRouteView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let mut route_memberships = groups::route_group_map(&mut tx).await?;
        let mut query = ModelRouteRow::all();
        if let Some(id) = self.user_id {
            query = query.filter(ModelRouteRow::fields().owner_user_id().eq(id));
        }
        if let Some(id) = group {
            self.for_group(id).require_group(&mut tx).await?;
            query = query.filter(
                ModelRouteRow::fields().id().in_list(
                    route_memberships
                        .iter()
                        .filter(|(_, groups)| groups.contains(&id))
                        .map(|(route, _)| *route)
                        .collect::<Vec<_>>(),
                ),
            );
        }
        let rows = query
            .order_by(ModelRouteRow::fields().id().asc())
            .exec(&mut tx)
            .await?;
        let mut targets = route_target_groups(&mut tx).await?;
        let (mappings, providers) = target_models(&mut tx, targets.values().flatten()).await?;
        let model_memberships = groups::model_group_map(&mut tx).await?;
        let mut routes = Vec::with_capacity(rows.len());
        for row in rows {
            let mut view = route_view_with_targets(
                &row,
                targets.remove(&row.id).unwrap_or_default(),
                &mappings,
                &providers,
            )?;
            view.group_ids = route_memberships.remove(&row.id).unwrap_or_default();
            for target in &mut view.targets {
                target.model.group_ids = model_memberships
                    .get(&target.model.id)
                    .cloned()
                    .unwrap_or_default();
            }
            routes.push(view);
        }
        tx.commit().await?;
        Ok(routes)
    }

    pub async fn create_route(&self, input: ModelRouteInput) -> StoreResult<ModelRouteView> {
        let input = validate_route(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        for target in &input.targets {
            let model = Box::pin(self.find_model(&mut tx, target.model_id)).await?;
            let provider = find(&mut tx, model.provider_id).await?;
            if provider.owner_user_id != self.user_id {
                return Err(StoreError::Validation("候选模型必须属于同一账户".into()));
            }
        }
        check_route_targets(&mut tx, input.provider_protocol, &input.targets).await?;
        let row = ModelRouteRow::create()
            .owner_user_id(self.user_id)
            .name(input.name)
            .protocol(input.protocol.as_str().to_owned())
            .provider_protocol(input.provider_protocol.as_str().to_owned())
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
        let mut row = self.find_route(&mut tx, id).await?;
        check_route_version(&row, version)?;
        for group_id in groups::route_groups(&mut tx, id).await? {
            check_unique_route_name(&mut tx, &input.name, group_id, input.protocol, Some(id))
                .await?;
            check_model_alias_available(&mut tx, &input.name, group_id).await?;
        }
        for target in &input.targets {
            let model = Box::pin(self.find_model(&mut tx, target.model_id)).await?;
            let provider = find(&mut tx, model.provider_id).await?;
            if provider.owner_user_id != row.owner_user_id {
                return Err(StoreError::Validation("候选模型必须属于同一账户".into()));
            }
        }
        check_route_targets(&mut tx, input.provider_protocol, &input.targets).await?;
        ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().route_id().eq(id))
            .delete()
            .exec(&mut tx)
            .await?;
        row.update()
            .name(input.name)
            .protocol(input.protocol.as_str().to_owned())
            .provider_protocol(input.provider_protocol.as_str().to_owned())
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
        let row = self.find_route(&mut tx, id).await?;
        check_route_version(&row, version)?;
        ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().route_id().eq(id))
            .delete()
            .exec(&mut tx)
            .await?;
        for group_id in groups::route_groups(&mut tx, id).await? {
            Self::touch_group(&mut tx, group_id).await?;
        }
        row.delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn route_view(tx: &mut Transaction<'_>, row: &ModelRouteRow) -> StoreResult<ModelRouteView> {
    let targets = ModelRouteTargetRow::all()
        .filter(ModelRouteTargetRow::fields().route_id().eq(row.id))
        .order_by(ModelRouteTargetRow::fields().position().asc())
        .exec(&mut *tx)
        .await?;
    let (mappings, providers) = target_models(tx, targets.iter()).await?;
    let mut view = route_view_with_targets(row, targets, &mappings, &providers)?;
    view.group_ids = groups::route_groups(tx, row.id).await?;
    for target in &mut view.targets {
        target.model.group_ids = groups::model_groups(tx, target.model.id).await?;
    }
    Ok(view)
}

pub(super) async fn route_target_groups(
    executor: &mut dyn Executor,
) -> StoreResult<HashMap<i64, Vec<ModelRouteTargetRow>>> {
    let targets = ModelRouteTargetRow::all()
        .order_by(ModelRouteTargetRow::fields().position().asc())
        .exec(executor)
        .await?;
    let mut groups: HashMap<i64, Vec<ModelRouteTargetRow>> = HashMap::new();
    for target in targets {
        groups.entry(target.route_id).or_default().push(target);
    }
    Ok(groups)
}

fn route_view_with_targets(
    row: &ModelRouteRow,
    targets: Vec<ModelRouteTargetRow>,
    mappings: &HashMap<i64, ModelMapping>,
    providers: &HashMap<i64, Provider>,
) -> StoreResult<ModelRouteView> {
    let mut views = Vec::with_capacity(targets.len());
    for target in targets {
        let mapping = mappings.get(&target.model_id).ok_or(StoreError::NotFound)?;
        let provider = providers
            .get(&mapping.provider_id)
            .ok_or(StoreError::NotFound)?;
        views.push(ModelRouteTargetView {
            model: mapping_view(mapping, provider)?,
            enabled: target.enabled,
        });
    }
    Ok(ModelRouteView {
        group_ids: Vec::new(),
        id: row.id,
        name: row.name.clone(),
        protocol: protocol_from_str(&row.protocol)?,
        provider_protocol: protocol_from_str(&row.provider_protocol)?,
        enabled: row.enabled,
        targets: views,
        version: row.version,
    })
}

async fn target_models<'a>(
    executor: &mut dyn Executor,
    targets: impl IntoIterator<Item = &'a ModelRouteTargetRow>,
) -> StoreResult<(HashMap<i64, ModelMapping>, HashMap<i64, Provider>)> {
    let ids: HashSet<_> = targets.into_iter().map(|target| target.model_id).collect();
    let mappings = if ids.is_empty() {
        Vec::new()
    } else {
        ModelMapping::all()
            .filter(
                ModelMapping::fields()
                    .id()
                    .in_list(ids.into_iter().collect::<Vec<_>>()),
            )
            .exec(executor)
            .await?
    };
    let providers =
        load_providers(executor, mappings.iter().map(|mapping| mapping.provider_id)).await?;
    Ok((
        mappings
            .into_iter()
            .map(|mapping| (mapping.id, mapping))
            .collect(),
        providers,
    ))
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
    let ids = targets
        .iter()
        .map(|target| target.model_id)
        .collect::<Vec<_>>();
    let protocols: HashMap<_, _> = ModelMapping::all()
        .filter(ModelMapping::fields().id().in_list(ids))
        .select((
            ModelMapping::fields().id(),
            ModelMapping::fields().openai_chat(),
            ModelMapping::fields().openai_responses(),
            ModelMapping::fields().anthropic_messages(),
            ModelMapping::fields().gemini(),
        ))
        .exec(tx)
        .await?
        .into_iter()
        .map(|(id, chat, responses, messages, gemini)| (id, (chat, responses, messages, gemini)))
        .collect();
    for target in targets {
        let (chat, responses, messages, gemini) = protocols
            .get(&target.model_id)
            .ok_or(StoreError::NotFound)?;
        let supported = match protocol {
            Protocol::OpenAiChat => *chat,
            Protocol::OpenAiResponses => *responses,
            Protocol::AnthropicMessages => *messages,
            Protocol::Gemini => *gemini,
        };
        if !supported {
            return Err(StoreError::Validation("候选模型不支持路由协议".into()));
        }
    }
    Ok(())
}

pub(super) async fn check_unique_route_name(
    tx: &mut Transaction<'_>,
    name: &str,
    group_id: i64,
    protocol: Protocol,
    own_id: Option<i64>,
) -> StoreResult<()> {
    let mut query = ModelRouteRow::all()
        .filter(ModelRouteRow::fields().name().eq(name))
        .filter(
            ModelRouteRow::fields()
                .id()
                .in_list(groups::route_ids(tx, group_id).await?),
        )
        .filter(ModelRouteRow::fields().protocol().eq(protocol.as_str()));
    if let Some(id) = own_id {
        query = query.filter(ModelRouteRow::fields().id().ne(id));
    }
    if query
        .select(ModelRouteRow::fields().id())
        .first()
        .exec(tx)
        .await?
        .is_some()
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

pub(super) async fn check_model_alias_available(
    tx: &mut Transaction<'_>,
    name: &str,
    group_id: i64,
) -> StoreResult<()> {
    if ModelMapping::all()
        .filter(ModelMapping::fields().alias().eq(name))
        .filter(
            ModelMapping::fields()
                .id()
                .in_list(groups::model_ids(tx, group_id).await?),
        )
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
