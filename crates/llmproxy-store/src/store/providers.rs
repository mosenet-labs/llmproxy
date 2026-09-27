use super::*;

impl ProviderStore {
    pub async fn list(&self) -> StoreResult<Vec<ProviderView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let providers = Provider::all()
            .order_by(Provider::fields().id().asc())
            .exec(&mut tx)
            .await?;
        let views = providers
            .iter()
            .map(|p| p.view(active_protocols(&bindings, p.id)))
            .collect();
        tx.commit().await?;
        views
    }

    pub async fn get(&self, id: i64) -> StoreResult<ProviderView> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let provider = find(&mut tx, id).await?;
        let view = provider.view(active_protocols(&bindings, id))?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn create(&self, input: ProviderInput) -> StoreResult<ProviderView> {
        self.create_with_probe_status(input, ProbeStatus::Unprobed)
            .await
    }

    pub async fn create_with_probe_status(
        &self,
        input: ProviderInput,
        status: ProbeStatus,
    ) -> StoreResult<ProviderView> {
        let input = validate(input, true)?;
        let encrypted_key = self.cipher.encrypt(&input.api_key)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        check_unique_name(&mut tx, &input.name, None).await?;
        let provider = Provider::create()
            .name(input.name)
            .openai_chat_path(input.paths.openai_chat)
            .openai_responses_path(input.paths.openai_responses)
            .anthropic_messages_path(input.paths.anthropic_messages)
            .host(input.host)
            .port(input.port)
            .tls(input.tls)
            .encrypted_key(encrypted_key)
            .enabled(input.enabled)
            .models_path(input.models_path)
            .models_protocol(input.models_protocol.as_str())
            .models_probe_status(status.as_str())
            .anthropic_version(input.anthropic_version)
            .connect_timeout_ms(input.connect_timeout_ms)
            .read_timeout_ms(input.read_timeout_ms)
            .write_timeout_ms(input.write_timeout_ms)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        let view = provider.view(Vec::new())?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn update(
        &self,
        id: i64,
        version: u64,
        input: ProviderInput,
    ) -> StoreResult<ProviderView> {
        self.update_with_probe_status(id, version, input, None)
            .await
    }

    pub async fn update_with_probe_status(
        &self,
        id: i64,
        version: u64,
        input: ProviderInput,
        status: Option<ProbeStatus>,
    ) -> StoreResult<ProviderView> {
        let input = validate(input, false)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        check_unique_name(&mut tx, &input.name, Some(id)).await?;
        if ModelMapping::all()
            .exec(&mut tx)
            .await?
            .iter()
            .any(|mapping| {
                mapping.provider_id == id
                    && mapping
                        .protocols()
                        .into_iter()
                        .any(|protocol| input.paths.get(protocol).is_none())
            })
        {
            return Err(StoreError::Conflict(
                "已有模型使用此协议，请先调整或删除模型映射".into(),
            ));
        }
        let paths = input.paths.clone();
        let encrypted_key = self
            .cipher
            .replacement(&input.api_key, &provider.encrypted_key)?;
        let probe_changed = input.host != provider.host
            || input.port != provider.port
            || input.tls != provider.tls
            || input.paths != provider.paths()
            || input.models_path != provider.models_path
            || input.models_protocol.as_str() != provider.models_protocol
            || (!input.api_key.is_empty())
            || input.anthropic_version != provider.anthropic_version;
        let probe_status = status
            .map(|status| status.as_str().to_owned())
            .unwrap_or_else(|| {
                if probe_changed {
                    ProbeStatus::Unprobed.as_str().to_owned()
                } else {
                    provider.models_probe_status.clone()
                }
            });
        provider
            .update()
            .name(input.name)
            .openai_chat_path(input.paths.openai_chat)
            .openai_responses_path(input.paths.openai_responses)
            .anthropic_messages_path(input.paths.anthropic_messages)
            .host(input.host)
            .port(input.port)
            .tls(input.tls)
            .encrypted_key(encrypted_key)
            .enabled(input.enabled)
            .models_path(input.models_path)
            .models_protocol(input.models_protocol.as_str())
            .models_probe_status(probe_status)
            .anthropic_version(input.anthropic_version)
            .connect_timeout_ms(input.connect_timeout_ms)
            .read_timeout_ms(input.read_timeout_ms)
            .write_timeout_ms(input.write_timeout_ms)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        if !provider.enabled {
            unbind(&mut tx, &mut bindings, id).await?;
        } else {
            for binding in &mut bindings {
                if binding.provider_id == Some(id)
                    && paths.get(protocol(&binding.protocol)?).is_none()
                {
                    binding.update().provider_id(None).exec(&mut tx).await?;
                }
            }
        }
        let view = provider.view(active_protocols(&bindings, id))?;
        tx.commit().await?;
        Ok(view)
    }

    pub async fn set_enabled(&self, id: i64, version: u64, enabled: bool) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        provider
            .update()
            .enabled(enabled)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        if !enabled {
            unbind(&mut tx, &mut bindings, id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn activate(&self, id: i64, version: u64, protocol: Protocol) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        if !provider.enabled {
            return Err(StoreError::Conflict(
                "请先启用此 Provider，再设为当前路由".into(),
            ));
        }
        if provider.paths().get(protocol).is_none() {
            return Err(StoreError::Validation("此 Provider 未配置该协议".into()));
        }
        // Validate decryptability before replacing the previous working binding.
        self.cipher.decrypt(&provider.encrypted_key)?;
        let binding = bindings
            .iter_mut()
            .find(|b| b.protocol == protocol.as_str())
            .ok_or(StoreError::Internal)?;
        if let Some(previous_id) = binding.provider_id.filter(|previous| *previous != id) {
            // Changing another provider's visible active state must invalidate its
            // stale edit form, too.
            let mut previous = find(&mut tx, previous_id).await?;
            previous.update().updated_at(now()?).exec(&mut tx).await?;
        }
        provider.update().updated_at(now()?).exec(&mut tx).await?;
        binding.update().provider_id(Some(id)).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, id: i64, version: u64) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let provider = find(&mut tx, id).await?;
        check_version(&provider, version)?;
        if ModelMapping::all()
            .exec(&mut tx)
            .await?
            .iter()
            .any(|mapping| mapping.provider_id == id)
        {
            return Err(StoreError::Conflict(
                "此 Provider 仍有模型映射，请先删除模型".into(),
            ));
        }
        if provider.enabled {
            return Err(StoreError::Conflict(
                "Provider 仍处于启用状态，请先停用后再删除".into(),
            ));
        }
        unbind(&mut tx, &mut bindings, id).await?;
        provider.delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn load_active(&self) -> StoreResult<Vec<ActiveProvider>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        let bindings = self.bindings(&mut tx, false).await?;
        let mut active = Vec::with_capacity(3);
        for binding in bindings {
            let Some(id) = binding.provider_id else {
                continue;
            };
            let provider = find(&mut tx, id).await?;
            let protocol = protocol(&binding.protocol)?;
            let Some(upstream_path) = provider.paths().get(protocol).map(str::to_owned) else {
                // Treat inconsistent storage as a failed snapshot, not partial config.
                return Err(StoreError::Internal);
            };
            if !provider.enabled {
                return Err(StoreError::Internal);
            }
            active.push(ActiveProvider {
                id,
                protocol,
                upstream_path,
                host: provider.host,
                port: provider.port,
                tls: provider.tls,
                secret: self.cipher.decrypt(&provider.encrypted_key)?,
                anthropic_version: provider.anthropic_version,
                connect_timeout_ms: provider.connect_timeout_ms,
                read_timeout_ms: provider.read_timeout_ms,
                write_timeout_ms: provider.write_timeout_ms,
            });
        }
        tx.commit().await?;
        Ok(active)
    }

    pub async fn probe_target(&self, id: i64) -> StoreResult<ModelProbeTarget> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let provider = find(&mut tx, id).await?;
        let target = ModelProbeTarget {
            host: provider.host,
            port: provider.port,
            tls: provider.tls,
            path: provider.models_path,
            protocol: protocol(&provider.models_protocol)?,
            secret: self.cipher.decrypt(&provider.encrypted_key)?,
            anthropic_version: provider.anthropic_version,
        };
        tx.commit().await?;
        Ok(target)
    }

    pub async fn probe_enabled_target(&self, id: i64) -> StoreResult<ModelProbeTarget> {
        if !self.get(id).await?.enabled {
            return Err(StoreError::Conflict("请先启用 Provider".into()));
        }
        self.probe_target(id).await
    }

    pub async fn preview_target(
        &self,
        id: Option<i64>,
        input: ProviderInput,
    ) -> StoreResult<ModelProbeTarget> {
        let input = validate(input, id.is_none())?;
        let secret = if input.api_key.is_empty() {
            let id = id.ok_or(StoreError::Internal)?;
            self.probe_target(id).await?.secret
        } else {
            input.api_key
        };
        Ok(ModelProbeTarget {
            host: input.host,
            port: input.port,
            tls: input.tls,
            path: input.models_path,
            protocol: input.models_protocol,
            secret,
            anthropic_version: input.anthropic_version,
        })
    }
}
