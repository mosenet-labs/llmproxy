//! 请求内暂存工具片段，数据库负责跨请求的加密保存和过期。

use super::{Result, failure};
use llmproxy_core::protocol::gemini::request::message::Part;
use llmproxy_store::{ProviderStore, StoreError, StoreResult, ToolContinuation};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

pub(super) use llmproxy_core::ir::message::TOOL_CONTINUATION_ID_PREFIX as PREFIX;
// 无数据库的同步单测沿用原内存期限；生产跨请求期限由存储层的 24 小时规则管理。
const TTL: Duration = Duration::from_secs(30 * 60);
const MAX_CALLS: usize = 4096;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// 保存原 Provider 片段、作用域及并行调用的组内位置。
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Entry {
    #[serde(skip)]
    pub scope: [u8; 32],
    pub group: [u8; 16],
    pub ordinal: usize,
    pub count: usize,
    pub part: Part,
    #[serde(skip, default = "expires")]
    pub expires: Instant,
    #[serde(skip)]
    pub bytes: usize,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    bytes: usize,
    created: HashSet<String>,
    touched: HashSet<String>,
}

/// 同步回调只访问当前请求的暂存；数据库操作在异步请求阶段或父请求中完成。
#[derive(Default)]
pub struct Cache {
    state: Mutex<State>,
    store: Option<ProviderStore>,
}

impl State {
    /// 每次访问清理过期项，不创建与请求脱离的后台任务。
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, entry| entry.expires > now);
        self.bytes = self.entries.values().map(|entry| entry.bytes).sum();
    }
}

impl Cache {
    /// 复用现有数据库连接池及主密钥；不建立额外的线程或数据库连接池。
    pub fn database(store: ProviderStore) -> Self {
        Self {
            state: Mutex::default(),
            store: Some(store),
        }
    }

    /// 每次请求隔离暂存，不以进程内旧条目代替数据库的真实有效期。
    pub(super) fn for_request(self: &std::sync::Arc<Self>) -> std::sync::Arc<Self> {
        match &self.store {
            Some(store) => std::sync::Arc::new(Self::database(store.clone())),
            None => self.clone(),
        }
    }

    /// 在恢复调用之前批量读取本轮引用，签名通过存储层解密。
    pub(super) async fn load(&self, ids: &[String], scope: &[u8; 32]) -> StoreResult<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let records = store
            .load_tool_continuations(ids, &scope_key(scope))
            .await?;
        let mut entries = Vec::with_capacity(records.len());
        for record in records {
            let mut entry: Entry =
                serde_json::from_str(&record.payload).map_err(|_| StoreError::Internal)?;
            let group = scope_key(&entry.group);
            if entry.count == 0
                || entry.ordinal >= entry.count
                || record.id != format!("{PREFIX}{group}_{}", entry.ordinal)
            {
                return Err(StoreError::Internal);
            }
            entry.scope = *scope;
            entry.bytes = record.payload.len() + record.id.len() + 128;
            entries.push((record.id, entry));
        }
        let mut state = self.state.lock().expect("tool state mutex");
        state.bytes = entries.iter().map(|(_, entry)| entry.bytes).sum();
        state.entries.extend(entries);
        Ok(())
    }

    /// 调用校验成功后才刷新数据库有效期，不在同步正文回调中等待 I/O。
    pub(super) async fn persist_touches(&self, scope: &[u8; 32]) -> StoreResult<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let ids: Vec<_> = self
            .state
            .lock()
            .expect("tool state mutex")
            .touched
            .iter()
            .cloned()
            .collect();
        store
            .touch_tool_continuations(&ids, &scope_key(scope))
            .await?;
        self.state.lock().expect("tool state mutex").touched.clear();
        Ok(())
    }

    /// 父请求发送真实响应头之前保存新引用，保存失败时客户端不会收到无效 ID。
    pub(super) async fn persist_created(&self) -> StoreResult<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let records = {
            let state = self.state.lock().expect("tool state mutex");
            state
                .created
                .iter()
                .map(|id| {
                    let entry = &state.entries[id];
                    Ok(ToolContinuation {
                        id: id.clone(),
                        scope: scope_key(&entry.scope),
                        payload: serde_json::to_string(entry).map_err(|_| StoreError::Internal)?,
                    })
                })
                .collect::<StoreResult<Vec<_>>>()?
        };
        store.save_tool_continuations(&records).await?;
        self.state.lock().expect("tool state mutex").created.clear();
        Ok(())
    }

    /// 响应编码成功后整组提交；容量不足不驱逐仍可用于后续回合的签名。
    pub(super) fn commit(&self, mut pending: Vec<(String, Entry)>) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        for (_, entry) in &mut pending {
            entry.expires = expires();
        }
        let bytes: usize = pending.iter().map(|(_, entry)| entry.bytes).sum();
        let mut state = self.state.lock().expect("tool state mutex");
        state.prune(Instant::now());
        if state.entries.len().saturating_add(pending.len()) > MAX_CALLS
            || state.bytes.saturating_add(bytes) > MAX_BYTES
            || pending.iter().any(|(id, _)| state.entries.contains_key(id))
        {
            return Err(failure("capacity"));
        }
        state.bytes += bytes;
        if self.store.is_some() {
            state
                .created
                .extend(pending.iter().map(|(id, _)| id.clone()));
        }
        state.entries.extend(pending);
        Ok(())
    }

    /// 读取不消费状态，允许原历史在后续多轮请求中重复出现。
    pub(super) fn get(&self, id: &str, scope: &[u8; 32]) -> Result<Entry> {
        let mut state = self.state.lock().expect("tool state mutex");
        state.prune(Instant::now());
        let entry = state
            .entries
            .get(id)
            .ok_or_else(|| failure("missing_or_expired"))?;
        if &entry.scope != scope {
            return Err(failure("scope_mismatch"));
        }
        Ok(entry.clone())
    }

    /// 整个请求验证通过后刷新滑动 TTL；失败请求不会续期。
    pub(super) fn touch(&self, ids: impl Iterator<Item = String>) {
        let mut state = self.state.lock().expect("tool state mutex");
        let now = Instant::now();
        state.prune(now);
        let expires = now + TTL;
        for id in ids {
            if let Some(entry) = state.entries.get_mut(&id) {
                entry.expires = expires;
                if self.store.is_some() {
                    state.touched.insert(id);
                }
            }
        }
    }
}

/// 引用组和鉴权摘要使用固定长度十六进制编码，不记录原始数据。
fn scope_key(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 每轮生成随机引用；不使用可推测的请求序号或 Provider 调用 ID。
pub(super) fn group() -> Result<([u8; 16], String)> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| failure("randomness"))?;
    let id = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((bytes, id))
}

/// 新状态从发布时开始计时。
pub(super) fn expires() -> Instant {
    Instant::now() + TTL
}

#[cfg(test)]
pub(super) fn expire(cache: &Cache) {
    for entry in cache.state.lock().unwrap().entries.values_mut() {
        entry.expires = Instant::now();
    }
}
