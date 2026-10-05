//! 工具续接状态与 Provider 配置共享数据库及主密钥，支持进程重启和多实例。

use super::*;
use crate::ToolContinuation;
use toasty_core::stmt::Value;

#[cfg(test)]
mod tests;

// 客户端协议没有统一的会话结束通知；以 24 小时闲置期限回收状态。
const RETENTION_SECONDS: i64 = 24 * 60 * 60;
const MAX_CALLS: i64 = 4096;
const MAX_BYTES: i64 = 16 * 1024 * 1024;

impl ProviderStore {
    /// 一次事务保存整轮调用；容量不足不驱逐其他仍有效的会话。
    pub async fn save_tool_continuations(&self, states: &[ToolContinuation]) -> StoreResult<()> {
        if states.is_empty() {
            return Ok(());
        }
        let mut ids = HashSet::new();
        let mut encrypted = Vec::with_capacity(states.len());
        let mut incoming_bytes = 0_i64;
        for state in states {
            if state.id.is_empty()
                || state.id.len() > 64
                || state.scope.len() != 64
                || !ids.insert(&state.id)
                || state.payload.len() > MAX_BYTES as usize
            {
                return Err(StoreError::Validation("工具续接状态无效或超出容量".into()));
            }
            let payload = self
                .cipher
                .encrypt_bound(&state.payload, &associated(state))?;
            incoming_bytes = incoming_bytes
                .checked_add(payload.len() as i64)
                .ok_or(StoreError::Internal)?;
            encrypted.push(payload);
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.lock_tool_continuations(&mut tx).await?;
        let now = now()?;
        for row in ToolContinuationRow::all()
            .filter(ToolContinuationRow::fields().expires_at().le(now))
            .exec(&mut tx)
            .await?
        {
            row.delete().exec(&mut tx).await?;
        }
        let counts = toasty::sql::query(
            "SELECT COUNT(*), CAST(COALESCE(SUM(payload_bytes), 0) AS BIGINT) FROM tool_continuations"
        ).exec(&mut tx).await?;
        let record = counts
            .first()
            .and_then(Value::as_record)
            .ok_or(StoreError::Internal)?;
        let [Value::I64(count), Value::I64(bytes)] = record.fields.as_slice() else {
            return Err(StoreError::Internal);
        };
        if count.saturating_add(states.len() as i64) > MAX_CALLS
            || bytes.saturating_add(incoming_bytes) > MAX_BYTES
        {
            return Err(StoreError::Conflict("工具续接状态存储容量不足".into()));
        }
        for (state, payload) in states.iter().zip(encrypted) {
            ToolContinuationRow::create()
                .id(state.id.clone())
                .scope(state.scope.clone())
                .payload_bytes(payload.len() as i64)
                .encrypted_payload(payload)
                .expires_at(now + RETENTION_SECONDS)
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// 按完整引用集合读取，缺失、过期或跨作用域均不返回部分状态，也不续期。
    pub async fn load_tool_continuations(
        &self,
        ids: &[String],
        scope: &str,
    ) -> StoreResult<Vec<ToolContinuation>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.verify_tool_key(&mut tx).await?;
        let rows = continuation_rows(&mut tx, ids, scope, now()?).await?;
        let mut states = Vec::with_capacity(rows.len());
        for row in rows {
            let mut state = ToolContinuation {
                id: row.id,
                scope: row.scope,
                payload: String::new(),
            };
            state.payload = self
                .cipher
                .decrypt_bound(&row.encrypted_payload, &associated(&state))?;
            states.push(state);
        }
        tx.commit().await?;
        Ok(states)
    }

    /// 只有调用及并行组校验成功才续期；过期状态不能被旧请求复活。
    pub async fn touch_tool_continuations(&self, ids: &[String], scope: &str) -> StoreResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.lock_tool_continuations(&mut tx).await?;
        let now = now()?;
        for mut row in continuation_rows(&mut tx, ids, scope, now).await? {
            let expires_at = row.expires_at.max(now + RETENTION_SECONDS);
            row.update().expires_at(expires_at).exec(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// PostgreSQL 锁定固定主密钥行，SQLite 使用 IMMEDIATE 事务串行容量检查。
    async fn lock_tool_continuations(&self, tx: &mut Transaction<'_>) -> StoreResult<()> {
        if !self.backend.is_sqlite() {
            toasty::sql::query("SELECT id FROM store_keys WHERE id = 1 FOR UPDATE")
                .exec(tx)
                .await?;
        }
        self.verify_tool_key(tx).await
    }

    /// 防止使用错误主密钥续期或写入；不在错误消息中暴露密文和载荷。
    async fn verify_tool_key(&self, tx: &mut Transaction<'_>) -> StoreResult<()> {
        let key = StoreKey::filter_by_id(1_i64)
            .first()
            .exec(tx)
            .await?
            .ok_or(MASTER_KEY_ERROR)?;
        self.verify(&key)
    }
}

/// 同一事务校验完整集合；重复引用只读取一次，调用重复由 Gateway 校验。
async fn continuation_rows(
    tx: &mut Transaction<'_>,
    ids: &[String],
    scope: &str,
    now: i64,
) -> StoreResult<Vec<ToolContinuationRow>> {
    let unique: HashSet<_> = ids.iter().collect();
    let rows = ToolContinuationRow::all()
        .filter(ToolContinuationRow::fields().id().in_list(ids.to_vec()))
        .filter(ToolContinuationRow::fields().scope().eq(scope.to_owned()))
        .filter(ToolContinuationRow::fields().expires_at().gt(now))
        .exec(tx)
        .await?;
    if rows.len() != unique.len() {
        return Err(StoreError::NotFound);
    }
    Ok(rows)
}

/// 标识长度固定上限，分隔符不会出现在 Gateway 生成的引用和十六进制作用域中。
fn associated(state: &ToolContinuation) -> Vec<u8> {
    format!("llmproxy.tool-continuation.v1:{}:{}", state.id, state.scope).into_bytes()
}
