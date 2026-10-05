//! 有界、进程内的工具状态；签名与鉴权摘要均不实现 Debug 或序列化。

use super::{Result, failure};
use llmproxy_core::protocol::gemini::request::message::Part;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

pub(super) const PREFIX: &str = "call_lp_";
const TTL: Duration = Duration::from_secs(30 * 60);
const MAX_CALLS: usize = 4096;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// 保存原 Provider 片段、作用域及并行调用的组内位置。
#[derive(Clone)]
pub(super) struct Entry {
    pub scope: [u8; 32],
    pub group: [u8; 16],
    pub ordinal: usize,
    pub count: usize,
    pub part: Part,
    pub expires: Instant,
    pub bytes: usize,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    bytes: usize,
}

/// 所有路由共享同一个有界存储；作用域摘要在读取时校验。
#[derive(Default)]
pub struct Cache {
    state: Mutex<State>,
}

impl State {
    /// 每次访问清理过期项，不创建与请求脱离的后台任务。
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, entry| entry.expires > now);
        self.bytes = self.entries.values().map(|entry| entry.bytes).sum();
    }
}

impl Cache {
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
            }
        }
    }
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
