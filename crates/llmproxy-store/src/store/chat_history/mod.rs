//! 会话事务与历史读取；字段分工防止 Console 覆盖来源统计。
use super::*;
use crate::chat_history::{
    self, ActualCall, Completion, Conversation, Input, Status, Turn, UsageState,
};
use llmproxy_core::{
    conversation::{Reply, Selection},
    ir::usage::Usage,
    protocol::Protocol,
    thinking::Choice,
};
use serde::{Serialize, de::DeserializeOwned};

mod retry;
pub(super) use retry::PendingUsage;
#[cfg(test)]
mod tests;

/// 只接受服务端生成的随机标识，也保护下面的锁查询。
fn id(value: &str) -> StoreResult<()> {
    if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(StoreError::Validation("历史记录标识无效".into()));
    }
    Ok(())
}
fn json<T: Serialize>(value: &T) -> StoreResult<String> {
    serde_json::to_string(value).map_err(|_| StoreError::Internal)
}
fn parse<T: DeserializeOwned>(value: &str) -> StoreResult<T> {
    serde_json::from_str(value).map_err(|_| StoreError::Internal)
}
fn associated(id: &str, field: &str) -> Vec<u8> {
    format!("llmproxy.chat.v1:{id}:{field}").into_bytes()
}
fn conversation(cipher: &KeyCipher, row: ChatConversationRow) -> StoreResult<Conversation> {
    Ok(Conversation {
        id: row.id.clone(),
        title: cipher.decrypt_bound(&row.title, &associated(&row.id, "title"))?,
        selection: parse(&row.selection_json)?,
        thinking: Choice::parse(&row.thinking).ok_or(StoreError::Internal)?,
        active_turn: row.active_turn,
        archived: row.archived,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}
impl ProviderStore {
    /// PostgreSQL 锁定会话行；SQLite 写事务使用 IMMEDIATE。
    async fn chat_row(
        &self,
        tx: &mut Transaction<'_>,
        key: &str,
    ) -> StoreResult<ChatConversationRow> {
        id(key)?;
        if !self.backend.is_sqlite() {
            toasty::sql::query(format!(
                "SELECT id FROM chat_conversations WHERE id = '{key}' FOR UPDATE"
            ))
            .exec(&mut *tx)
            .await?;
        }
        let row = ChatConversationRow::filter_by_id(key)
            .first()
            .exec(&mut *tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if row.owner != chat_history::OWNER {
            return Err(StoreError::NotFound);
        }
        Ok(row)
    }
    /// 创建空会话；正式记录在用户提交前已存在。
    pub async fn create_chat_conversation(
        &self,
        key: &str,
        selection: &Selection,
    ) -> StoreResult<Conversation> {
        id(key)?;
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let timestamp = now()?;
        let row = ChatConversationRow::create()
            .id(key)
            .owner(chat_history::OWNER)
            .title(self.cipher.encrypt_bound("", &associated(key, "title"))?)
            .selection_json(json(selection)?)
            .thinking("default")
            .active_turn(None::<String>)
            .archived(false)
            .created_at(timestamp)
            .updated_at(timestamp)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        conversation(&self.cipher, row)
    }
    /// 分页读取未归档历史，页面初始化不会自动进入归档会话。
    pub async fn list_chat_conversations(
        &self,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<Conversation>> {
        self.chat_conversations(false, offset, limit).await
    }
    /// 归档使用相同分页与解密规则，不删除内容和用量。
    pub async fn list_archived_chat_conversations(
        &self,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<Conversation>> {
        self.chat_conversations(true, offset, limit).await
    }
    /// 两类历史共用查询；排序使用 ID 补充相同时间的稳定顺序。
    async fn chat_conversations(
        &self,
        archived: bool,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<Conversation>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.verify_tool_key(&mut tx).await?;
        let rows = ChatConversationRow::all()
            .filter(
                ChatConversationRow::fields()
                    .owner()
                    .eq(chat_history::OWNER),
            )
            .filter(ChatConversationRow::fields().archived().eq(archived))
            .order_by(ChatConversationRow::fields().updated_at().desc())
            .order_by(ChatConversationRow::fields().id().desc())
            .limit(limit.min(100))
            .offset(offset)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|row| conversation(&self.cipher, row))
            .collect()
    }
    /// 加载旧会话，不依赖现存模型配置。
    pub async fn get_chat_conversation(&self, key: &str) -> StoreResult<Conversation> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.verify_tool_key(&mut tx).await?;
        let row = ChatConversationRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if row.owner != chat_history::OWNER {
            return Err(StoreError::NotFound);
        }
        tx.commit().await?;
        conversation(&self.cipher, row)
    }
    /// 下一轮偏好只在会话空闲时修改，数据库校验覆盖多页面并发。
    pub async fn select_chat(
        &self,
        key: &str,
        selection: &Selection,
        thinking: Choice,
    ) -> StoreResult<()> {
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let mut row = self.chat_row(&mut tx, key).await?;
        if row.archived {
            return Err(StoreError::Conflict("会话已归档，请恢复后继续".into()));
        }
        if row.active_turn.is_some() {
            return Err(StoreError::Conflict(
                "正在生成回复，请先停止或等待完成".into(),
            ));
        }
        row.update()
            .selection_json(json(selection)?)
            .thinking(thinking.as_str())
            .updated_at(now()?)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// 输入与生成中轮次原子提交，锁内分配会话顺序。
    pub async fn begin_chat_turn(&self, input: Input) -> StoreResult<Turn> {
        id(&input.id)?;
        if input.prompt.trim().is_empty() || input.prompt.chars().count() > 4000 {
            return Err(StoreError::Validation("输入为空或过长".into()));
        }
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let mut room = self.chat_row(&mut tx, &input.conversation_id).await?;
        if room.archived {
            return Err(StoreError::Conflict("会话已归档，请恢复后继续".into()));
        }
        if let Some(existing) = ChatTurnRow::filter_by_id(&input.id)
            .first()
            .exec(&mut tx)
            .await?
        {
            if existing.conversation_id != input.conversation_id
                || existing.selection_json != json(&input.selection)?
                || existing.alias != input.alias
                || existing.thinking != input.thinking.as_str()
                || self.cipher.decrypt_bound(
                    &existing.encrypted_prompt,
                    &associated(&existing.id, "prompt"),
                )? != input.prompt
            {
                return Err(StoreError::Conflict("轮次标识已经被使用".into()));
            }
            tx.commit().await?;
            return self.turn(existing);
        }
        if room.active_turn.is_some() {
            return Err(StoreError::Conflict(
                "正在生成回复，请先停止或等待完成".into(),
            ));
        }
        if room.selection_json != json(&input.selection)?
            || room.thinking != input.thinking.as_str()
        {
            return Err(StoreError::Conflict("模型选择已改变，请重试".into()));
        }
        let previous = ChatTurnRow::all()
            .filter(
                ChatTurnRow::fields()
                    .conversation_id()
                    .eq(&input.conversation_id),
            )
            .order_by(ChatTurnRow::fields().sequence().desc())
            .limit(1)
            .exec(&mut tx)
            .await?;
        let sequence = previous.first().map_or(0, |r| r.sequence) + 1;
        let timestamp = now()?;
        let encrypted = self
            .cipher
            .encrypt_bound(&input.prompt, &associated(&input.id, "prompt"))?;
        let row = ChatTurnRow::create()
            .id(input.id.clone())
            .conversation_id(input.conversation_id)
            .sequence(sequence)
            .selection_json(json(&input.selection)?)
            .alias(input.alias)
            .thinking(input.thinking.as_str())
            .encrypted_prompt(encrypted)
            .encrypted_reply(None::<String>)
            .status("generating")
            .error(None::<String>)
            .actual_json(None::<String>)
            .usage_json(None::<String>)
            .usage_state("unreported")
            .gateway_started(false)
            .gateway_finished(false)
            .provider_id(None::<i64>)
            .upstream_model(None::<String>)
            .input_tokens(None::<u64>)
            .output_tokens(None::<u64>)
            .total_tokens(None::<u64>)
            .cache_read_tokens(None::<u64>)
            .cache_write_tokens(None::<u64>)
            .reasoning_tokens(None::<u64>)
            .started_at(timestamp)
            .ended_at(None::<i64>)
            .exec(&mut tx)
            .await?;
        let old_title = self
            .cipher
            .decrypt_bound(&room.title, &associated(&room.id, "title"))?;
        let title = if old_title.is_empty() {
            let first: String = input.prompt.chars().take(24).collect();
            if input.prompt.chars().count() > 24 {
                format!("{first}…")
            } else {
                first
            }
        } else {
            old_title
        };
        let encrypted_title = self
            .cipher
            .encrypt_bound(&title, &associated(&room.id, "title"))?;
        room.update()
            .active_turn(Some(input.id))
            .title(encrypted_title)
            .selection_json(json(&input.selection)?)
            .thinking(input.thinking.as_str())
            .updated_at(timestamp)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        self.turn(row)
    }
    /// 只更新 Console 拥有的字段，先到的来源统计始终保留。
    pub async fn finish_chat_turn(&self, key: &str, completion: Completion) -> StoreResult<()> {
        if completion.status == Status::Generating {
            return Err(StoreError::Validation("轮次结束状态无效".into()));
        }
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        let mut room = self.chat_row(&mut tx, &row.conversation_id).await?;
        let mut row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if row.status != "generating" {
            tx.commit().await?;
            return Ok(());
        }
        let envelope = ReplyEnvelope {
            version: 1,
            reply: completion.reply,
        };
        let encrypted = self
            .cipher
            .encrypt_bound(&json(&envelope)?, &associated(key, "reply"))?;
        let error = completion
            .error
            .map(|text| self.cipher.encrypt_bound(&text, &associated(key, "error")))
            .transpose()?;
        let timestamp = now()?;
        row.update()
            .encrypted_reply(Some(encrypted))
            .status(completion.status.as_str())
            .error(error)
            .ended_at(Some(timestamp))
            .exec(&mut tx)
            .await?;
        room.update()
            .active_turn(None::<String>)
            .updated_at(timestamp)
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// 内部认证后只允许已有轮次认领一次实际请求，并核对入口模型和协议。
    pub async fn start_chat_call(
        &self,
        key: &str,
        alias: &str,
        protocol: Protocol,
        actual: &ActualCall,
    ) -> StoreResult<()> {
        id(key)?;
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.chat_row(&mut tx, &row.conversation_id).await?;
        let mut row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        let selection: Selection = parse(&row.selection_json)?;
        if row.gateway_started
            || row.status != "generating"
            || row.alias != alias
            || selection.protocol != protocol
        {
            return Err(StoreError::Conflict("历史请求关联无效或重复".into()));
        }
        row.update()
            .gateway_started(true)
            .actual_json(Some(json(actual)?))
            .provider_id(Some(actual.provider_id))
            .upstream_model(Some(actual.upstream_model.clone()))
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// 来源 Usage 完整快照只提交一次，不将客户端协议统计写回此记录。
    pub async fn finish_chat_call(
        &self,
        key: &str,
        actual: &ActualCall,
        usage: Option<&Usage>,
        state: UsageState,
    ) -> StoreResult<()> {
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.chat_row(&mut tx, &row.conversation_id).await?;
        let mut row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut tx)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !row.gateway_started || row.gateway_finished {
            tx.commit().await?;
            return Ok(());
        }
        row.update()
            .gateway_finished(true)
            .actual_json(Some(json(actual)?))
            .usage_json(usage.map(json).transpose()?)
            .usage_state(state.as_str())
            .input_tokens(usage.and_then(|u| u.input_tokens))
            .output_tokens(usage.and_then(|u| u.output_tokens))
            .total_tokens(usage.and_then(|u| u.total_tokens))
            .cache_read_tokens(usage.and_then(|u| u.cache.read_input_tokens))
            .cache_write_tokens(usage.and_then(|u| u.cache.write_input_tokens))
            .reasoning_tokens(usage.and_then(|u| u.output_details.reasoning_tokens))
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// 有序加载类型化内容，密钥错误或损坏载荷不能降级为文本历史。
    pub async fn chat_turns(&self, conversation_id: &str) -> StoreResult<Vec<Turn>> {
        self.get_chat_conversation(conversation_id).await?;
        let mut connection = self.connection().await?;
        let rows = ChatTurnRow::all()
            .filter(ChatTurnRow::fields().conversation_id().eq(conversation_id))
            .order_by(ChatTurnRow::fields().sequence().asc())
            .exec(&mut connection)
            .await?;
        rows.into_iter().map(|row| self.turn(row)).collect()
    }
    /// 读取当前轮的权威统计，生成结束与网关保存可以先后到达。
    pub async fn chat_turn(&self, key: &str) -> StoreResult<Turn> {
        let mut connection = self.connection().await?;
        let row = ChatTurnRow::filter_by_id(key)
            .first()
            .exec(&mut connection)
            .await?
            .ok_or(StoreError::NotFound)?;
        self.turn(row)
    }
    /// 启动恢复单个本地服务遗留的轮次，不自动重新调用 Provider。
    pub async fn recover_chat_history(&self) -> StoreResult<()> {
        let mut connection = self.connection().await?;
        let rows = ChatConversationRow::all()
            .filter(
                ChatConversationRow::fields()
                    .owner()
                    .eq(chat_history::OWNER),
            )
            .exec(&mut connection)
            .await?;
        drop(connection);
        let rooms: Vec<_> = rows
            .into_iter()
            .map(|row| conversation(&self.cipher, row))
            .collect::<StoreResult<_>>()?;
        for room in rooms {
            if let Some(key) = room.active_turn {
                let turn = self.chat_turn(&key).await?;
                if let Some(actual) = &turn.actual {
                    self.finish_chat_call(
                        &key,
                        actual,
                        turn.usage.as_ref(),
                        if turn.usage.is_some() {
                            UsageState::Partial
                        } else {
                            UsageState::Unreported
                        },
                    )
                    .await?;
                }
                self.finish_chat_turn(
                    &key,
                    Completion {
                        status: Status::Interrupted,
                        reply: Reply::default(),
                        error: Some("服务重启，生成已中断".into()),
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
    /// 归档／恢复只更新会话状态，行锁确保不能与生成开始同时成功。
    pub async fn set_chat_archived(&self, key: &str, archived: bool) -> StoreResult<()> {
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let mut room = self.chat_row(&mut tx, key).await?;
        if room.active_turn.is_some() {
            return Err(StoreError::Conflict(
                "正在生成回复，请先停止或等待完成".into(),
            ));
        }
        if room.archived != archived {
            room.update()
                .archived(archived)
                .updated_at(now()?)
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// 删除空闲会话及其轮次；配置记录和其他会话不受影响。
    pub async fn delete_chat_conversation(&self, key: &str) -> StoreResult<()> {
        let _writer = if self.backend.is_sqlite() {
            Some(self.chat_writes.lock().await)
        } else {
            None
        };
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.verify_tool_key(&mut tx).await?;
        let room = self.chat_row(&mut tx, key).await?;
        if room.active_turn.is_some() {
            return Err(StoreError::Conflict(
                "正在生成回复，请先停止或等待完成".into(),
            ));
        }
        let mut removed = Vec::new();
        for row in ChatTurnRow::all()
            .filter(ChatTurnRow::fields().conversation_id().eq(key))
            .exec(&mut tx)
            .await?
        {
            removed.push(row.id.clone());
            row.delete().exec(&mut tx).await?;
        }
        room.delete().exec(&mut tx).await?;
        tx.commit().await?;
        let mut pending = self.pending_chat_usage.lock().expect("pending usage mutex");
        for key in removed {
            pending.remove(&key);
        }
        Ok(())
    }
    /// 密文只在 Store 边界解码，来源统计与页面快照保持独立。
    fn turn(&self, row: ChatTurnRow) -> StoreResult<Turn> {
        let reply = row
            .encrypted_reply
            .as_deref()
            .map(|text| -> StoreResult<Reply> {
                let decoded = self
                    .cipher
                    .decrypt_bound(text, &associated(&row.id, "reply"))?;
                let envelope: ReplyEnvelope = parse(&decoded)?;
                if envelope.version != 1 {
                    return Err(StoreError::Configuration("历史内容版本不支持"));
                }
                Ok(envelope.reply)
            })
            .transpose()?;
        Ok(Turn {
            call_started: row.gateway_started,
            call_finished: row.gateway_finished,
            id: row.id.clone(),
            conversation_id: row.conversation_id,
            sequence: row.sequence,
            selection: parse(&row.selection_json)?,
            alias: row.alias,
            thinking: Choice::parse(&row.thinking).ok_or(StoreError::Internal)?,
            prompt: self
                .cipher
                .decrypt_bound(&row.encrypted_prompt, &associated(&row.id, "prompt"))?,
            reply,
            status: parse(&json(&row.status)?)?,
            error: row
                .error
                .as_deref()
                .map(|text| {
                    self.cipher
                        .decrypt_bound(text, &associated(&row.id, "error"))
                })
                .transpose()?,
            actual: row.actual_json.as_deref().map(parse).transpose()?,
            usage: row.usage_json.as_deref().map(parse).transpose()?,
            usage_state: parse(&json(&row.usage_state)?)?,
            started_at: row.started_at,
            ended_at: row.ended_at,
        })
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct ReplyEnvelope {
    version: u32,
    reply: Reply,
}
