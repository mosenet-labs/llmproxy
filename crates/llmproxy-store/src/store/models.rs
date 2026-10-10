use super::*;

pub(super) async fn check_route_name_available(
    tx: &mut Transaction<'_>,
    name: &str,
    group_id: i64,
) -> StoreResult<()> {
    if ModelRouteRow::all()
        .filter(ModelRouteRow::fields().name().eq(name))
        .filter(
            ModelRouteRow::fields()
                .id()
                .in_list(groups::route_ids(tx, group_id).await?),
        )
        .select(ModelRouteRow::fields().id())
        .first()
        .exec(tx)
        .await?
        .is_some()
    {
        return Err(StoreError::Conflict("模型标识与已有路由名称重复".into()));
    }
    Ok(())
}

impl ProviderStore {
    /// Call entries explicitly selected for this group (Chat and gateway scope).
    pub async fn list_models(&self) -> StoreResult<Vec<ModelMappingView>> {
        Box::pin(self.models_in_group(Some(self.group_id))).await
    }

    /// System catalog, including resources that have not been selected for any group.
    pub async fn list_all_models(&self) -> StoreResult<Vec<ModelMappingView>> {
        Box::pin(self.models_in_group(None)).await
    }

    async fn models_in_group(&self, group: Option<i64>) -> StoreResult<Vec<ModelMappingView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        if group.is_none() {
            self.require_space(&mut tx, true).await?;
        }
        let mut memberships = groups::model_group_map(&mut tx).await?;
        let mut query = ModelMapping::all();
        if self.space_id.is_some() {
            query = query.filter(
                ModelMapping::fields()
                    .provider_id()
                    .in_list(self.provider_ids(&mut tx).await?),
            );
        }
        if let Some(id) = group {
            self.for_group(id).require_group(&mut tx).await?;
            query = query.filter(
                ModelMapping::fields().id().in_list(
                    memberships
                        .iter()
                        .filter(|(_, groups)| groups.contains(&id))
                        .map(|(model, _)| *model)
                        .collect::<Vec<_>>(),
                ),
            );
        }
        let mappings = query
            .order_by(ModelMapping::fields().id().asc())
            .exec(&mut tx)
            .await?;
        let providers =
            load_providers(&mut tx, mappings.iter().map(|mapping| mapping.provider_id)).await?;
        let mut result = Vec::with_capacity(mappings.len());
        for mapping in mappings {
            let mut view = mapping_view(
                &mapping,
                providers
                    .get(&mapping.provider_id)
                    .ok_or(StoreError::NotFound)?,
            )?;
            view.group_ids = memberships.remove(&mapping.id).unwrap_or_default();
            result.push(view);
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Read catalog configuration independently of group call permissions.
    pub async fn get_model(&self, id: i64) -> StoreResult<ModelMappingView> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        if self.authorized_group_ids(&mut tx).await?.is_some() {
            self.require_group(&mut tx).await?;
            self.require_model_member(&mut tx, id).await?;
        }
        let mapping = Box::pin(self.find_model(&mut tx, id)).await?;
        let provider = Box::pin(self.find_provider(&mut tx, mapping.provider_id)).await?;
        let mut view = mapping_view(&mapping, &provider)?;
        view.group_ids = groups::model_groups(&mut tx, mapping.id).await?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn create_model(&self, input: ModelMappingInput) -> StoreResult<ModelMappingView> {
        let input = validate_mapping(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let provider = Box::pin(self.find_provider(&mut tx, input.provider_id)).await?;
        check_mapping_provider(&provider, &input)?;
        let upstream_model_id = input.upstream_model_id.clone();
        let catalog_price = input.reference_price.clone();
        let mapping = ModelMapping::create()
            .thinking_json(toasty::Json(&input.thinking))
            .alias(input.alias)
            .provider_id(input.provider_id)
            .upstream_model_id(input.upstream_model_id)
            .openai_chat(input.protocols.contains(&Protocol::OpenAiChat))
            .openai_responses(input.protocols.contains(&Protocol::OpenAiResponses))
            .anthropic_messages(input.protocols.contains(&Protocol::AnthropicMessages))
            .gemini(input.protocols.contains(&Protocol::Gemini))
            .input_price_per_million(
                input
                    .reference_price
                    .as_ref()
                    .map(|price| price.input_per_million.clone()),
            )
            .output_price_per_million(
                input
                    .reference_price
                    .as_ref()
                    .map(|price| price.output_per_million.clone()),
            )
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        if let Some(price) = &catalog_price {
            pricing::seed_catalog_price(&mut tx, provider.id, &upstream_model_id, price).await?;
        }
        let view = mapping_view(&mapping, &provider)?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn create_models(
        &self,
        inputs: Vec<ModelMappingInput>,
    ) -> StoreResult<Vec<ModelMappingView>> {
        let Some(first) = inputs.first() else {
            return Err(StoreError::Validation("请至少选择一个模型".into()));
        };
        let provider_id = first.provider_id;
        let mut aliases = HashSet::new();
        let mut model_ids = HashSet::new();
        let mut validated = Vec::with_capacity(inputs.len());
        for input in inputs {
            let label = input.upstream_model_id.clone();
            let input = validate_mapping(input)
                .map_err(|error| StoreError::Validation(format!("「{label}」：{error}")))?;
            if input.provider_id != provider_id {
                return Err(StoreError::Validation(
                    "一次只能导入同一 Provider 的模型".into(),
                ));
            }
            if !aliases.insert(input.alias.clone()) {
                return Err(StoreError::Conflict(format!("「{label}」：模型标识重复")));
            }
            if !model_ids.insert(input.upstream_model_id.clone()) {
                return Err(StoreError::Conflict(format!("「{label}」：模型重复选择")));
            }
            validated.push(input);
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let provider = Box::pin(self.find_provider(&mut tx, provider_id)).await?;
        let mut saved = Vec::with_capacity(validated.len());
        for input in validated {
            let label = input.upstream_model_id.clone();
            check_mapping_provider(&provider, &input)
                .map_err(|error| StoreError::Validation(format!("「{label}」：{error}")))?;
            let upstream_model_id = input.upstream_model_id.clone();
            let catalog_price = input.reference_price.clone();
            let mapping = ModelMapping::create()
                .thinking_json(toasty::Json(&input.thinking))
                .alias(input.alias)
                .provider_id(provider_id)
                .upstream_model_id(input.upstream_model_id)
                .openai_chat(input.protocols.contains(&Protocol::OpenAiChat))
                .openai_responses(input.protocols.contains(&Protocol::OpenAiResponses))
                .anthropic_messages(input.protocols.contains(&Protocol::AnthropicMessages))
                .gemini(input.protocols.contains(&Protocol::Gemini))
                .input_price_per_million(
                    input
                        .reference_price
                        .as_ref()
                        .map(|price| price.input_per_million.clone()),
                )
                .output_price_per_million(
                    input
                        .reference_price
                        .as_ref()
                        .map(|price| price.output_per_million.clone()),
                )
                .updated_at(now()?)
                .exec(&mut tx)
                .await?;
            if let Some(price) = &catalog_price {
                pricing::seed_catalog_price(&mut tx, provider_id, &upstream_model_id, price)
                    .await?;
            }
            let view = mapping_view(&mapping, &provider)?;
            saved.push(view);
        }
        tx.commit().await?;
        Ok(saved)
    }

    pub async fn update_model(
        &self,
        id: i64,
        version: u64,
        input: ModelMappingInput,
    ) -> StoreResult<ModelMappingView> {
        let input = validate_mapping(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut mapping = Box::pin(self.find_model(&mut tx, id)).await?;
        check_mapping_version(&mapping, version)?;
        let previous = find(&mut tx, mapping.provider_id).await?;
        let provider = Box::pin(self.find_provider(&mut tx, input.provider_id)).await?;
        if provider.space_id != previous.space_id {
            return Err(StoreError::Validation(
                "不能将模型移动到其他账户的 Provider".into(),
            ));
        }
        check_mapping_provider(&provider, &input)?;
        for target in ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().model_id().eq(id))
            .exec(&mut tx)
            .await?
        {
            let route = ModelRouteRow::filter_by_id(target.route_id)
                .first()
                .exec(&mut tx)
                .await?
                .ok_or(StoreError::Internal)?;
            if !input
                .protocols
                .contains(&routes::protocol_from_str(&route.provider_protocol)?)
            {
                return Err(StoreError::Conflict(
                    "模型仍被此协议的路由使用，请先从路由中移除".into(),
                ));
            }
        }
        for group_id in groups::model_groups(&mut tx, id).await? {
            check_unique_alias(&mut tx, &input.alias, group_id, Some(id)).await?;
            check_route_name_available(&mut tx, &input.alias, group_id).await?;
        }
        mapping
            .update()
            .thinking_json(toasty::Json(&input.thinking))
            .alias(input.alias)
            .provider_id(input.provider_id)
            .upstream_model_id(input.upstream_model_id)
            .openai_chat(input.protocols.contains(&Protocol::OpenAiChat))
            .openai_responses(input.protocols.contains(&Protocol::OpenAiResponses))
            .anthropic_messages(input.protocols.contains(&Protocol::AnthropicMessages))
            .gemini(input.protocols.contains(&Protocol::Gemini))
            .input_price_per_million(
                input
                    .reference_price
                    .as_ref()
                    .map(|price| price.input_per_million.clone()),
            )
            .output_price_per_million(
                input
                    .reference_price
                    .as_ref()
                    .map(|price| price.output_per_million.clone()),
            )
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        let mut view = mapping_view(&mapping, &provider)?;
        view.group_ids = groups::model_groups(&mut tx, mapping.id).await?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn delete_model(&self, id: i64, version: u64) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mapping = Box::pin(self.find_model(&mut tx, id)).await?;
        check_mapping_version(&mapping, version)?;
        let references = ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().model_id().eq(id))
            .select(ModelRouteTargetRow::fields().id())
            .first()
            .exec(&mut tx)
            .await?;
        if references.is_some() {
            return Err(StoreError::Conflict(
                "此模型仍被模型路由使用，请先从路由中移除".into(),
            ));
        }
        for group_id in groups::model_groups(&mut tx, id).await? {
            Self::touch_group(&mut tx, group_id).await?;
        }
        mapping.delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_model_routes(&self) -> StoreResult<Vec<ModelRoute>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.require_space(&mut tx, true).await?;
        self.bindings(&mut tx, false).await?;
        Box::pin(self.require_group(&mut tx)).await?;
        let mut routes = Vec::new();
        let mut route_names = HashSet::new();
        let mut target_groups = routes::route_target_groups(&mut tx).await?;
        let mapping_rows = ModelMapping::all().exec(&mut tx).await?;
        let providers = load_providers(
            &mut tx,
            mapping_rows.iter().map(|mapping| mapping.provider_id),
        )
        .await?;
        let mappings: HashMap<_, _> = mapping_rows
            .iter()
            .map(|mapping| (mapping.id, mapping))
            .collect();
        for route in ModelRouteRow::all()
            .filter(
                ModelRouteRow::fields()
                    .id()
                    .in_list(groups::route_ids(&mut tx, self.group_id).await?),
            )
            .exec(&mut tx)
            .await?
        {
            let protocol = routes::protocol_from_str(&route.protocol)?;
            let provider_protocol = routes::protocol_from_str(&route.provider_protocol)?;
            route_names.insert((route.name.clone(), protocol));
            let targets = target_groups.remove(&route.id).unwrap_or_default();
            let mut candidates = Vec::with_capacity(targets.len());
            for target in &targets {
                let mapping = mappings.get(&target.model_id).ok_or(StoreError::NotFound)?;
                let provider = providers
                    .get(&mapping.provider_id)
                    .ok_or(StoreError::NotFound)?;
                candidates.push((target, mapping, provider));
            }
            let selected = candidates.iter().find(|(target, mapping, provider)| {
                route.enabled
                    && target.enabled
                    && provider.enabled
                    && mapping.protocols().contains(&provider_protocol)
            });
            let resolved = if let Some((_, _, provider)) = selected {
                Some(
                    self.active_provider(
                        provider,
                        provider_protocol,
                        provider
                            .paths()
                            .get(provider_protocol)
                            .ok_or(StoreError::Internal)?
                            .to_owned(),
                    )?,
                )
            } else {
                None
            };
            routes.push(ModelRoute {
                model_id: selected.map(|(_, mapping, _)| mapping.id),
                thinking: selected
                    .map(|(_, mapping, _)| mapping.thinking())
                    .transpose()?
                    .unwrap_or_default(),
                alias: route.name.clone(),
                upstream_model_id: selected.map_or(String::new(), |(_, mapping, _)| {
                    mapping.upstream_model_id.clone()
                }),
                enabled: selected.is_some(),
                provider: resolved,
                protocol,
            });
        }
        let member_ids = groups::model_ids(&mut tx, self.group_id).await?;
        for mapping in mapping_rows
            .iter()
            .filter(|mapping| member_ids.contains(&mapping.id))
        {
            let provider = providers
                .get(&mapping.provider_id)
                .ok_or(StoreError::NotFound)?;
            for protocol in mapping.protocols() {
                if route_names.contains(&(mapping.alias.clone(), protocol)) {
                    continue;
                }
                let resolved = if provider.enabled {
                    Some(
                        self.active_provider(
                            provider,
                            protocol,
                            provider
                                .paths()
                                .get(protocol)
                                .ok_or(StoreError::Internal)?
                                .to_owned(),
                        )?,
                    )
                } else {
                    None
                };
                routes.push(ModelRoute {
                    model_id: Some(mapping.id),
                    thinking: mapping.thinking()?,
                    alias: mapping.alias.clone(),
                    upstream_model_id: mapping.upstream_model_id.clone(),
                    enabled: provider.enabled,
                    provider: resolved,
                    protocol,
                });
            }
        }
        tx.commit().await?;
        Ok(routes)
    }

    pub async fn load_model_route(&self, id: i64, protocol: Protocol) -> StoreResult<ModelRoute> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let mapping = Box::pin(self.find_model(&mut tx, id)).await?;
        self.require_model_member(&mut tx, mapping.id).await?;
        if !mapping.protocols().contains(&protocol) {
            return Err(StoreError::Validation("模型未配置所选协议".into()));
        }
        let provider = Box::pin(self.find_provider(&mut tx, mapping.provider_id)).await?;
        let upstream_path = provider
            .paths()
            .get(protocol)
            .ok_or(StoreError::Internal)?
            .to_owned();
        let resolved = if provider.enabled {
            Some(self.active_provider(&provider, protocol, upstream_path)?)
        } else {
            None
        };
        tx.commit().await?;
        Ok(ModelRoute {
            model_id: Some(mapping.id),
            thinking: mapping.thinking()?,
            alias: mapping.alias,
            upstream_model_id: mapping.upstream_model_id,
            enabled: provider.enabled,
            provider: resolved,
            protocol,
        })
    }
}
