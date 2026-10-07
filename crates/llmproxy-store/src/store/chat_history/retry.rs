//! 来源统计保存失败时保留同一终态快照，页面重试无需再次调用 Provider。
use super::*;

#[derive(Clone)]
pub(in crate::store) struct PendingUsage {
    failed: bool,
    actual: ActualCall,
    usage: Option<Usage>,
    state: UsageState,
}
impl ProviderStore {
    /// 入队后再执行短事务；暂时故障不会在请求退出时丢掉已采集用量。
    pub async fn save_chat_usage(
        &self,
        key: &str,
        actual: &ActualCall,
        usage: Option<&Usage>,
        state: UsageState,
    ) -> StoreResult<()> {
        id(key)?;
        self.pending_chat_usage
            .lock()
            .expect("pending usage mutex")
            .entry(key.into())
            .or_insert_with(|| PendingUsage {
                failed: false,
                actual: actual.clone(),
                usage: usage.cloned(),
                state,
            });
        self.retry_chat_usage(key).await
    }
    /// 重试使用已有业务标识和来源快照，数据库保存仍保持幂等。
    pub async fn retry_chat_usage(&self, key: &str) -> StoreResult<()> {
        let pending = self
            .pending_chat_usage
            .lock()
            .expect("pending usage mutex")
            .get(key)
            .cloned();
        if let Some(pending) = pending {
            match self
                .finish_chat_call(key, &pending.actual, pending.usage.as_ref(), pending.state)
                .await
            {
                Ok(()) | Err(StoreError::NotFound) => {
                    self.pending_chat_usage
                        .lock()
                        .expect("pending usage mutex")
                        .remove(key);
                }
                Err(error) => {
                    if let Some(entry) = self
                        .pending_chat_usage
                        .lock()
                        .expect("pending usage mutex")
                        .get_mut(key)
                    {
                        entry.failed = true;
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    /// 页面只公开保存状态，不公开来源统计的内部重试载荷。
    pub fn chat_usage_pending(&self, key: &str) -> bool {
        self.pending_chat_usage
            .lock()
            .expect("pending usage mutex")
            .get(key)
            .is_some_and(|entry| entry.failed)
    }
}
