use super::*;

async fn check_route_name_available(tx: &mut Transaction<'_>, name: &str) -> StoreResult<()> {
    if !ModelRouteRow::all()
        .filter(ModelRouteRow::fields().name().eq(name))
        .exec(tx)
        .await?
        .is_empty()
    {
        return Err(StoreError::Conflict("模型标识与已有路由名称重复".into()));
    }
    Ok(())
}

impl ProviderStore {
    pub async fn list_models(&self) -> StoreResult<Vec<ModelMappingView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let mappings = ModelMapping::all()
            .order_by(ModelMapping::fields().id().asc())
            .exec(&mut tx)
            .await?;
        let mut result = Vec::with_capacity(mappings.len());
        for mapping in mappings {
            let provider = find(&mut tx, mapping.provider_id).await?;
            result.push(mapping_view(&mapping, &provider));
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn get_model(&self, id: i64) -> StoreResult<ModelMappingView> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let mapping = find_mapping(&mut tx, id).await?;
        let provider = find(&mut tx, mapping.provider_id).await?;
        let view = mapping_view(&mapping, &provider);
        tx.commit().await?;
        Ok(view)
    }

    pub async fn create_model(&self, input: ModelMappingInput) -> StoreResult<ModelMappingView> {
        let input = validate_mapping(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let provider = find(&mut tx, input.provider_id).await?;
        check_mapping_provider(&provider, &input)?;
        check_unique_alias(&mut tx, &input.alias, None).await?;
        check_route_name_available(&mut tx, &input.alias).await?;
        let upstream_model_id = input.upstream_model_id.clone();
        let catalog_price = input.reference_price.clone();
        let mapping = ModelMapping::create()
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
        let view = mapping_view(&mapping, &provider);
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
        let provider = find(&mut tx, provider_id).await?;
        let mut saved = Vec::with_capacity(validated.len());
        for input in validated {
            let label = input.upstream_model_id.clone();
            check_mapping_provider(&provider, &input)
                .map_err(|error| StoreError::Validation(format!("「{label}」：{error}")))?;
            check_unique_alias(&mut tx, &input.alias, None)
                .await
                .map_err(|error| StoreError::Conflict(format!("「{label}」：{error}")))?;
            check_route_name_available(&mut tx, &input.alias)
                .await
                .map_err(|error| StoreError::Conflict(format!("「{label}」：{error}")))?;
            let upstream_model_id = input.upstream_model_id.clone();
            let catalog_price = input.reference_price.clone();
            let mapping = ModelMapping::create()
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
            saved.push(mapping_view(&mapping, &provider));
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
        let mut mapping = find_mapping(&mut tx, id).await?;
        check_mapping_version(&mapping, version)?;
        let provider = find(&mut tx, input.provider_id).await?;
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
        check_unique_alias(&mut tx, &input.alias, Some(id)).await?;
        if input.alias != mapping.alias {
            check_route_name_available(&mut tx, &input.alias).await?;
        }
        mapping
            .update()
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
        let view = mapping_view(&mapping, &provider);
        tx.commit().await?;
        Ok(view)
    }

    pub async fn delete_model(&self, id: i64, version: u64) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mapping = find_mapping(&mut tx, id).await?;
        check_mapping_version(&mapping, version)?;
        let references = ModelRouteTargetRow::all()
            .filter(ModelRouteTargetRow::fields().model_id().eq(id))
            .exec(&mut tx)
            .await?;
        if !references.is_empty() {
            return Err(StoreError::Conflict(
                "此模型仍被模型路由使用，请先从路由中移除".into(),
            ));
        }
        mapping.delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_model_routes(&self) -> StoreResult<Vec<ModelRoute>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let mut routes = Vec::new();
        let mut route_names = HashSet::new();
        for route in ModelRouteRow::all().exec(&mut tx).await? {
            let protocol = routes::protocol_from_str(&route.protocol)?;
            let provider_protocol = routes::protocol_from_str(&route.provider_protocol)?;
            route_names.insert((route.name.clone(), protocol));
            let targets = ModelRouteTargetRow::all()
                .filter(ModelRouteTargetRow::fields().route_id().eq(route.id))
                .order_by(ModelRouteTargetRow::fields().position().asc())
                .exec(&mut tx)
                .await?;
            let mut candidates = Vec::with_capacity(targets.len());
            for target in targets {
                let mapping = find_mapping(&mut tx, target.model_id).await?;
                let provider = find(&mut tx, mapping.provider_id).await?;
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
                alias: route.name.clone(),
                upstream_model_id: selected.map_or(String::new(), |(_, mapping, _)| {
                    mapping.upstream_model_id.clone()
                }),
                enabled: selected.is_some(),
                provider: resolved,
                protocol,
            });
        }
        for mapping in ModelMapping::all().exec(&mut tx).await? {
            let provider = find(&mut tx, mapping.provider_id).await?;
            for protocol in mapping.protocols() {
                if route_names.contains(&(mapping.alias.clone(), protocol)) {
                    continue;
                }
                let resolved = if provider.enabled {
                    Some(
                        self.active_provider(
                            &provider,
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
        let mapping = find_mapping(&mut tx, id).await?;
        if !mapping.protocols().contains(&protocol) {
            return Err(StoreError::Validation("模型未配置所选协议".into()));
        }
        let provider = find(&mut tx, mapping.provider_id).await?;
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
            alias: mapping.alias,
            upstream_model_id: mapping.upstream_model_id,
            enabled: provider.enabled,
            provider: resolved,
            protocol,
        })
    }
}
