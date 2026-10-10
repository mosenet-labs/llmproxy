//! Chat 应用服务：短事务协调输入保存、活跃会话与历史恢复。
use super::chat_sessions::{ChatSession, ChatSessions};
use llmproxy_core::{
    conversation::{Reply, Selection},
    thinking::Choice,
};
use llmproxy_store::{
    ProviderStore,
    chat_history::{Completion, Input, Status},
};
use std::{io, sync::Arc};

pub(crate) mod generation;
pub(crate) mod target;

/// 随机业务标识不随进程重启复用，独立于遥测自增 ID。
pub(crate) fn new_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| io::Error::other("无法创建历史记录标识"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
/// Store 克隆复用已有连接池；不在活跃状态锁内等待数据库。
pub(crate) struct ChatService<'a> {
    pub store: &'a ProviderStore,
    pub sessions: &'a ChatSessions,
}
/// 历史行操作；归档与删除只有在作用于当前会话时才切换工作区。
#[derive(Clone, Copy)]
pub(crate) enum HistoryAction {
    Archive,
    Restore,
    Delete,
}
impl ChatService<'_> {
    /// 新会话立即落库，稳定工作区归属允许刷新后查回。
    pub async fn create(&self, selection: Selection) -> topcoat::Result<String> {
        let id = new_id()?;
        let room = self.store.create_chat_conversation(&id, &selection).await?;
        self.sessions.restore(self.store.group_id(), &room, &[]);
        Ok(id)
    }
    /// 活跃会话复用内存，空闲历史首次访问从数据库恢复。
    pub async fn load(&self, id: &str) -> topcoat::Result<Arc<ChatSession>> {
        if let Some(room) = self.sessions.get(id) {
            if room.group_id != self.store.group_id() {
                return Err(topcoat::router::error::forbidden().into());
            }
            // 页面请求重新验证归属；内部生成任务在数据库故障时仍须保留待保存回复。
            if self.store.actor_user_id().is_some() {
                self.store.get_chat_conversation(id).await?;
            }
            return Ok(room);
        }
        let room = self.store.get_chat_conversation(id).await?;
        let turns = self.store.chat_turns(id).await?;
        Ok(self.sessions.restore(self.store.group_id(), &room, &turns))
    }
    /// 页面初始化恢复最近会话，没有记录时创建首个会话。
    pub async fn initial(&self, fallback: Selection) -> topcoat::Result<(String, Selection)> {
        let records = self.store.list_chat_conversations(0, 1).await?;
        if let Some(room) = records.first() {
            self.load(&room.id).await?;
            // 没有模型时创建的空会话，在首次配置模型后采用页面默认选择。
            if room.selection.model_id.is_empty()
                && !fallback.model_id.is_empty()
                && room.active_turn.is_none()
                && self.store.chat_turns(&room.id).await?.is_empty()
            {
                self.select(&room.id, fallback.clone(), room.thinking)
                    .await?;
                return Ok((room.id.clone(), fallback));
            }
            return Ok((room.id.clone(), room.selection.clone()));
        }
        let id = self.create(fallback.clone()).await?;
        Ok((id, fallback))
    }
    /// 输入事务成功之后才能调用 Provider；数据库互斥防止多页同时生成。
    pub async fn begin(&self, id: &str, prompt: &str, alias: &str) -> topcoat::Result<bool> {
        let room = self.load(id).await?;
        if room.snapshot().2 {
            return Ok(false);
        }
        let record = self
            .store
            .begin_chat_turn(Input {
                id: new_id()?,
                conversation_id: id.into(),
                selection: room.selection(),
                alias: alias.into(),
                thinking: room.thinking_choice(),
                prompt: prompt.trim().into(),
            })
            .await?;
        if !room.begin(prompt) {
            self.store
                .finish_chat_turn(
                    &record.id,
                    Completion {
                        status: Status::Failed,
                        reply: Reply::default(),
                        error: Some("会话状态已改变，请重试".into()),
                    },
                )
                .await?;
            return Ok(false);
        }
        room.attach_record(&record.id);
        Ok(true)
    }
    /// 保存完整或部分可见内容，只有有效完成回复允许进入下一轮上下文。
    pub async fn finish(&self, id: &str, result: Result<Reply, String>) -> topcoat::Result<()> {
        let room = self.load(id).await?;
        let (key, mut latest) = room.record_snapshot();
        let stopped = room.is_cancelled();
        let status = if stopped {
            Status::Cancelled
        } else if result.is_ok() {
            if result.as_ref().ok().and_then(|r| r.status)
                == Some(llmproxy_core::ir::response::Status::Incomplete)
            {
                Status::Incomplete
            } else {
                Status::Completed
            }
        } else {
            Status::Failed
        };
        let error = if stopped {
            Some("已停止生成".into())
        } else {
            result.as_ref().err().cloned()
        };
        if let Ok(reply) = &result {
            latest = reply.clone();
        }
        if !status.usable() {
            latest.history = Default::default();
            latest.parts.clear();
        }
        if let Some(key) = key {
            room.queue_save(
                key,
                Completion {
                    status,
                    reply: latest,
                    error,
                },
            );
        }
        room.finish(result);
        self.retry_save(id).await?;
        Ok(())
    }
    /// 数据库恢复后可显式重试，成功才清除“尚未保存”提示。
    pub async fn retry_save(&self, id: &str) -> topcoat::Result<()> {
        let room = self.load(id).await?;
        if let Some((key, completion)) = room.pending_save() {
            if self.store.finish_chat_turn(&key, completion).await.is_err() {
                room.save_error("本轮回复尚未保存，请重试保存");
                return Ok(());
            }
            room.saved();
        }
        let turns = match self.store.chat_turns(id).await {
            Ok(turns) => turns,
            Err(_) => {
                room.save_error("无法读取历史保存状态，请恢复数据库后重试保存");
                return Ok(());
            }
        };
        for turn in turns {
            if self.store.retry_chat_usage(&turn.id).await.is_err() {
                room.save_error("本轮来源用量尚未保存，请重试保存");
                return Ok(());
            }
        }
        room.saved();
        self.refresh(id).await?;
        Ok(())
    }
    /// 来源统计可能晚于正文结束，由权威记录更新本轮标签和计数。
    pub async fn refresh(&self, id: &str) -> topcoat::Result<bool> {
        let room = self.load(id).await?;
        let turns = self.store.chat_turns(id).await?;
        let mut pending = false;
        for turn in &turns {
            pending |= turn.call_started && !turn.call_finished;
            room.apply_record(turn);
        }
        Ok(pending)
    }
    /// 下一轮选择落库后更新活跃状态，历史轮次保持原快照。
    pub async fn select(
        &self,
        id: &str,
        selection: Selection,
        thinking: Choice,
    ) -> topcoat::Result<()> {
        let room = self.load(id).await?;
        self.store.select_chat(id, &selection, thinking).await?;
        if !room.select(selection.clone()) || !room.set_thinking(&selection, thinking) {
            return Err(io::Error::other("正在生成回复，请先停止或等待完成").into());
        }
        Ok(())
    }
    /// 删除空闲历史并淘汰活跃缓存，生成中删除由数据库拒绝。
    pub async fn delete(&self, id: &str) -> topcoat::Result<()> {
        self.store.delete_chat_conversation(id).await?;
        self.sessions.remove(id);
        Ok(())
    }
    /// 操作任意历史；离开当前会话时选择最近的未归档记录，必要时新建。
    pub async fn change_history(
        &self,
        current: &str,
        target: &str,
        action: HistoryAction,
    ) -> topcoat::Result<Option<(String, Selection)>> {
        let fallback = if current == target && !matches!(action, HistoryAction::Restore) {
            Some(self.load(current).await?.selection())
        } else {
            None
        };
        match action {
            HistoryAction::Delete => self.delete(target).await?,
            HistoryAction::Archive => self.store.set_chat_archived(target, true).await?,
            HistoryAction::Restore => self.store.set_chat_archived(target, false).await?,
        }
        match fallback {
            Some(selection) => self.initial(selection).await.map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;
