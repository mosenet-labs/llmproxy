//! 模拟与真实 Provider 共用的非流式验收设施。
pub mod fixtures;

use llmproxy_core::protocol::Protocol;
use llmproxy_store::{
    ModelMappingInput, ModelRouteInput, ModelRouteTargetInput, ProviderInput, ProviderStore,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];
pub const MASTER_KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// 所有测试路由都写入独立数据库；来源配置库始终只读。
pub struct Database {
    pub store: ProviderStore,
    pub url: String,
    directory: PathBuf,
}

impl Database {
    pub async fn new() -> Self {
        Self::with_master_key(MASTER_KEY).await
    }

    /// 真实 Provider 的临时凭据继续使用用户主密钥加密，不使用公开测试密钥。
    pub async fn with_master_key(master_key: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "llmproxy-matrix-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).unwrap();
        let url = format!("sqlite:{}", directory.join("providers.sqlite3").display());
        let store = ProviderStore::connect(&url, master_key).await.unwrap();
        store.migrate().await.unwrap();
        Self {
            store,
            url,
            directory,
        }
    }

    /// 为一个目标 Provider 建立四个客户端协议入口。
    pub async fn add_provider(&self, provider: ProviderInput, target: Protocol, model: &str) {
        let record = self
            .store
            .create(provider)
            .await
            .expect("创建隔离 Provider");
        let mapping = self
            .store
            .create_model(ModelMappingInput {
                alias: format!("internal-{}", target.as_str()),
                provider_id: record.id,
                upstream_model_id: model.into(),
                protocols: vec![target],
                reference_price: None,
            })
            .await
            .unwrap();
        for source in ALL {
            self.store
                .create_route(ModelRouteInput {
                    name: alias(source, target),
                    protocol: source,
                    provider_protocol: target,
                    enabled: true,
                    targets: vec![ModelRouteTargetInput {
                        model_id: mapping.id,
                        enabled: true,
                    }],
                })
                .await
                .unwrap();
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // 测试目录只保存本次生成的数据；不会删除来源 PostgreSQL 的任何内容。
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// 路由名称与模型别名分离，覆盖 Gateway 的模型重写。
pub fn alias(source: Protocol, target: Protocol) -> String {
    format!("matrix-{}-{}", source.as_str(), target.as_str())
}

/// Gemini 模型位于路径，其余协议模型位于请求正文。
pub fn path(protocol: Protocol, model: &str) -> String {
    if protocol == Protocol::Gemini {
        format!("/v1beta/models/{model}:generateContent")
    } else {
        protocol.upstream_path().into()
    }
}
