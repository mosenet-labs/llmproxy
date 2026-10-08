use super::*;
use crate::{SubscriptionNodeView, SubscriptionRelayTarget, model::SubscriptionNode};
use llmproxy_core::subscription::Registration;

async fn provider_name(
    tx: &mut dyn Executor,
    name: &str,
    custom: Option<&str>,
    own_id: Option<i64>,
) -> StoreResult<String> {
    if let Some(custom) = custom {
        match check_unique_name(tx, custom, own_id).await {
            Ok(()) => return Ok(custom.to_owned()),
            Err(StoreError::Conflict(_)) => {}
            Err(error) => return Err(error),
        }
    }
    check_unique_name(tx, name, own_id).await?;
    Ok(name.to_owned())
}

fn view(row: SubscriptionNode) -> StoreResult<SubscriptionNodeView> {
    Ok(SubscriptionNodeView {
        node_id: row.node_id,
        name: row.name,
        provider_name: row.provider_name,
        backend: row.backend,
        models: row.models_json.0,
        concurrency: row.concurrency,
        enabled: row.enabled,
        provider_id: row.provider_id,
        version: row.config_version,
    })
}

impl ProviderStore {
    pub async fn subscription_nodes(&self) -> StoreResult<Vec<SubscriptionNodeView>> {
        let mut connection = self.connection().await?;
        SubscriptionNode::all()
            .exec(&mut connection)
            .await?
            .into_iter()
            .map(view)
            .collect()
    }

    pub async fn register_subscription(
        &self,
        registration: &Registration,
    ) -> StoreResult<SubscriptionNodeView> {
        if registration.version != llmproxy_core::subscription::VERSION
            || registration.node_id.len() != 64
            || !registration.node_id.bytes().all(|b| b.is_ascii_hexdigit())
            || registration.node_key.len() != 64
            || !registration.node_key.bytes().all(|b| b.is_ascii_hexdigit())
            || registration.name.len() > 128
            || !matches!(registration.backend.as_str(), "codex" | "chatgpt-oauth")
            || registration.concurrency == 0
            || registration.concurrency > 64
            || registration.models.is_empty()
            || registration.models.len() > 256
            || registration
                .models
                .iter()
                .any(|m| m.is_empty() || m.len() > 256)
        {
            return Err(StoreError::Validation(
                "节点身份、协议版本或能力声明无效".into(),
            ));
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let row = if let Some(mut row) = SubscriptionNode::filter_by_node_id(&registration.node_id)
            .first()
            .exec(&mut tx)
            .await?
        {
            let secret = self.cipher.decrypt(&row.encrypted_node_key)?;
            let matching = secret.len() == registration.node_key.len()
                && secret
                    .bytes()
                    .zip(registration.node_key.bytes())
                    .fold(0, |difference, (a, b)| difference | (a ^ b))
                    == 0;
            if !matching || row.backend != registration.backend {
                return Err(StoreError::Conflict(
                    "节点身份认证失败或后端发生变化".into(),
                ));
            }
            row.update()
                .models_json(&registration.models)
                .concurrency(registration.concurrency as u64)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?;
            row
        } else {
            SubscriptionNode::create()
                .node_id(&registration.node_id)
                .name(if registration.name.trim().is_empty() {
                    format!("node-{}", &registration.node_id[..6])
                } else {
                    registration.name.trim().to_owned()
                })
                .provider_name(None)
                .config_version(0_u64)
                .backend(&registration.backend)
                .models_json(&registration.models)
                .concurrency(registration.concurrency as u64)
                .encrypted_node_key(self.cipher.encrypt(&registration.node_key)?)
                .enabled(false)
                .provider_id(None)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?
        };
        let row = view(row)?;
        tx.commit().await?;
        Ok(row)
    }

    pub async fn rename_subscription(
        &self,
        node_id: &str,
        version: u64,
        name: &str,
    ) -> StoreResult<()> {
        let name = name.trim();
        if name.len() > 128 {
            return Err(StoreError::Validation(
                "Provider 名称最多 128 字节，可留空".into(),
            ));
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        let mut node = SubscriptionNode::filter_by_node_id(node_id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if node.config_version != version {
            return Err(StoreError::Conflict("节点配置已变化，请刷新".into()));
        }
        let custom = (!name.is_empty()).then_some(name);
        let display_name = provider_name(&mut tx, &node.name, custom, node.provider_id).await?;
        if let Some(id) = node.provider_id {
            let mut provider = find(&mut tx, id).await?;
            provider
                .update()
                .name(display_name)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?;
        }
        let config_version = node.config_version + 1;
        node.update()
            .provider_name(custom.map(str::to_owned))
            .config_version(config_version)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_subscription_enabled(
        &self,
        node_id: &str,
        version: u64,
        enabled: bool,
        target: &SubscriptionRelayTarget,
    ) -> StoreResult<()> {
        llmproxy_core::provider::validate_upstream(
            &target.host,
            target.port,
            None,
            10000,
            310000,
            30000,
        )
        .map_err(|_| StoreError::Validation("转接地址无效".into()))?;
        if target.key.len() < 32 {
            return Err(StoreError::Configuration("未配置订阅转接密钥"));
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        let mut bindings = self.bindings(&mut tx, true).await?;
        let mut node = SubscriptionNode::filter_by_node_id(node_id)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if node.config_version != version {
            return Err(StoreError::Conflict("节点配置已变化，请刷新".into()));
        }
        let provider_id = if let Some(id) = node.provider_id {
            let display_name =
                provider_name(&mut tx, &node.name, node.provider_name.as_deref(), Some(id)).await?;
            let mut provider = find(&mut tx, id).await?;
            provider
                .update()
                .name(display_name)
                .enabled(enabled)
                .host(&target.host)
                .port(target.port)
                .tls(false)
                .openai_responses_path(Some(format!("/internal/subscriptions/{node_id}/responses")))
                .models_path(format!("/internal/subscriptions/{node_id}/models"))
                .models_protocol("openai_responses")
                .encrypted_key(self.cipher.encrypt(&target.key)?)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?;
            if !enabled {
                unbind(&mut tx, &mut bindings, id).await?;
            }
            Some(id)
        } else if enabled {
            let name =
                provider_name(&mut tx, &node.name, node.provider_name.as_deref(), None).await?;
            let provider = Provider::create()
                .name(name)
                .openai_chat_path(None)
                .openai_responses_path(Some(format!("/internal/subscriptions/{node_id}/responses")))
                .anthropic_messages_path(None)
                .gemini_path(None)
                .host(&target.host)
                .port(target.port)
                .tls(false)
                .encrypted_key(self.cipher.encrypt(&target.key)?)
                .enabled(true)
                .models_path(format!("/internal/subscriptions/{node_id}/models"))
                .models_protocol("openai_responses")
                .models_probe_status("unprobed")
                .anthropic_version(None)
                .messages_auth("x-api-key")
                .connect_timeout_ms(10000_u64)
                .read_timeout_ms(310000_u64)
                .write_timeout_ms(30000_u64)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?;
            Some(provider.id)
        } else {
            None
        };
        let config_version = node.config_version + 1;
        node.update()
            .config_version(config_version)
            .enabled(enabled)
            .provider_id(provider_id)
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
