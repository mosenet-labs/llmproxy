//! Gemini 工具回合状态；HTTP 接入层通过共享数据库保存和恢复 Provider 片段。
//! 参考：https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures

mod cache;
mod stream;
pub use stream::Capture as StreamCapture;
#[cfg(test)]
mod stream_tests;
#[cfg(test)]
mod tests;
use crate::snapshot::ResolvedProvider;
pub use cache::Cache;
use cache::{Entry, PREFIX};
use llmproxy_core::{
    adapter::Error,
    ir::{message::PartKind, response::Response as IrResponse},
    protocol::{OptionalNullable as O, Protocol, Request, gemini::response::Response},
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
type Result<T> = std::result::Result<T, Error>;

/// 单请求固定的路由与鉴权作用域，随父子请求上下文一起传递。
#[derive(Clone)]
pub struct Context {
    cache: Arc<Cache>,
    scope: [u8; 32],
}

impl Context {
    /// 摘要包含长度边界，避免字段拼接歧义；不保存或记录明文鉴权头。
    pub fn new(
        cache: Arc<Cache>,
        provider: &ResolvedProvider,
        model: &str,
        protocol: Protocol,
        route: &str,
        credentials: [&[u8]; 3],
    ) -> Self {
        let mut digest = Sha256::new();
        for value in [
            provider.host.as_bytes(),
            &provider.port.to_be_bytes(),
            &[u8::from(provider.tls)],
            provider.upstream_path.as_bytes(),
            provider.secret.as_bytes(),
            model.as_bytes(),
            protocol.as_str().as_bytes(),
            route.as_bytes(),
            credentials[0],
            credentials[1],
            credentials[2],
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
        Self {
            cache: cache.for_request(),
            scope: digest.finalize().into(),
        }
    }

    /// 类型化目标请求已构造完成后读取签名，全部验证成功才续期并发往 Provider。
    pub async fn restore_persisted(&self, request: &mut Request) -> pingora::Result<()> {
        let Request::Gemini(body) = request else {
            return Ok(());
        };
        let ids: std::collections::HashSet<_> = body
            .contents
            .iter()
            .flat_map(|message| &message.parts)
            .flat_map(|part| {
                [
                    part.function_call
                        .as_option()
                        .and_then(|call| call.id.as_option()),
                    part.function_response
                        .as_option()
                        .and_then(|result| result.id.as_option()),
                ]
            })
            .flatten()
            .filter(|id| id.starts_with(PREFIX))
            .cloned()
            .collect();
        let ids: Vec<_> = ids.into_iter().collect();
        database(self.cache.load(&ids, &self.scope), false).await?;
        self.restore(request).map_err(|_| {
            pingora::Error::explain(
                pingora::ErrorType::HTTPStatus(422),
                "invalid tool continuation",
            )
        })?;
        database(self.cache.persist_touches(&self.scope), false).await
    }

    /// 响应转换结束后由父请求调用，保存完成前不发送客户端响应头。
    pub async fn persist_response(&self) -> pingora::Result<()> {
        database(self.cache.persist_created(), true).await
    }

    /// 把带签名调用投影成客户端引用；返回待提交状态，编码失败不占用缓存。
    pub fn capture(
        &self,
        raw: &Response,
        ir: &mut IrResponse,
        target: Protocol,
    ) -> Result<Pending> {
        let mut pending = Vec::new();
        let selected = ir.candidates.iter().map(|candidate| candidate.index).min();
        let candidates = raw.candidates.as_option().into_iter().flatten().enumerate();
        let messages = candidates.filter_map(|(position, candidate)| {
            candidate.content.as_option().map(|message| {
                (
                    candidate
                        .index
                        .as_option()
                        .copied()
                        .unwrap_or(position as u64),
                    message,
                )
            })
        });
        for ((index, original), message) in messages.zip(&mut ir.messages) {
            if target != Protocol::OpenAiChat && Some(index) != selected {
                continue;
            }
            let originals: Vec<_> = original
                .parts
                .iter()
                .filter(|part| part.function_call.as_option().is_some())
                .collect();
            if !originals.iter().any(|part| {
                part.thought_signature
                    .as_option()
                    .is_some_and(|signature| !signature.is_empty())
            }) {
                continue;
            }
            let calls: Vec<_> = message
                .parts
                .iter_mut()
                .filter_map(|part| match &mut part.kind {
                    PartKind::ToolCall(call) => Some(call),
                    _ => None,
                })
                .collect();
            if calls.len() != originals.len() {
                return Err(failure("invalid_call_group"));
            }
            let group = self.capture_group(&originals)?;
            for (call, (id, _)) in calls.into_iter().zip(&group.entries) {
                call.id = Some(id.clone());
            }
            pending.extend(group.entries);
        }
        Ok(Pending {
            cache: self.cache.clone(),
            entries: pending,
        })
    }

    /// 非流式和流式共享完整并行调用组的引用、序号及容量计算。
    fn capture_group(
        &self,
        parts: &[&llmproxy_core::protocol::gemini::request::message::Part],
    ) -> Result<Pending> {
        let (group, token) = cache::group()?;
        let mut entries = Vec::with_capacity(parts.len());
        for (ordinal, part) in parts.iter().enumerate() {
            let id = format!("{PREFIX}{token}_{ordinal}");
            let bytes = serde_json::to_vec(part)?.len() + id.len() + 128;
            entries.push((
                id,
                Entry {
                    scope: self.scope,
                    group,
                    ordinal,
                    count: parts.len(),
                    part: (*part).clone(),
                    expires: cache::expires(),
                    bytes,
                },
            ));
        }
        Ok(Pending {
            cache: self.cache.clone(),
            entries,
        })
    }

    /// IR 已构造目标 struct 后恢复原片段，逐一校验名称、参数和并行组边界。
    pub fn restore(&self, request: &mut Request) -> Result<()> {
        let Request::Gemini(request) = request else {
            return Ok(());
        };
        let mut restored = HashMap::<String, Entry>::new();
        for message in &mut request.contents {
            let calls: Vec<_> = message
                .parts
                .iter()
                .enumerate()
                .filter_map(|(position, part)| {
                    part.function_call.as_option().map(|call| (position, call))
                })
                .collect();
            let mut entries = Vec::new();
            for (position, call) in &calls {
                if let Some(id) = call.id.as_option().filter(|id| id.starts_with(PREFIX)) {
                    let entry = self.cache.get(id, &self.scope)?;
                    let original = entry
                        .part
                        .function_call
                        .as_option()
                        .expect("cached function call");
                    if call.name != original.name
                        || normalized_args(&call.args) != normalized_args(&original.args)
                    {
                        return Err(failure("call_modified"));
                    }
                    entries.push((*position, id.clone(), entry));
                }
            }
            if entries.is_empty() {
                continue;
            }
            let first = &entries[0].2;
            if entries.len() != calls.len()
                || entries.len() != first.count
                || entries.iter().enumerate().any(|(ordinal, (_, _, entry))| {
                    entry.group != first.group || entry.ordinal != ordinal
                })
            {
                return Err(failure("invalid_call_group"));
            }
            for (position, id, entry) in entries {
                message.parts[position] = entry.part.clone();
                if restored.insert(id, entry).is_some() {
                    return Err(failure("duplicate_call"));
                }
            }
        }
        let mut results = HashMap::<[u8; 16], Vec<(usize, usize, usize)>>::new();
        for (message_index, message) in request.contents.iter_mut().enumerate() {
            for (part_index, part) in message.parts.iter_mut().enumerate() {
                if let O::Value(result) = &mut part.function_response
                    && let Some(id) = result.id.as_option().filter(|id| id.starts_with(PREFIX))
                {
                    let entry = restored.get(id).ok_or_else(|| failure("unpaired_result"))?;
                    let call = entry
                        .part
                        .function_call
                        .as_option()
                        .expect("cached function call");
                    if result.name != call.name {
                        return Err(failure("result_modified"));
                    }
                    result.id = call.id.clone();
                    results.entry(entry.group).or_default().push((
                        entry.ordinal,
                        message_index,
                        part_index,
                    ));
                }
            }
        }
        // 没有 Provider 调用 ID 的并行结果靠位置对应；允许客户端乱序完成，
        // 将完整结果片段按原调用顺序放回已有结果位置，正文不做合并或改写。
        for positions in results.values() {
            let mut ordered = positions.clone();
            ordered.sort_by_key(|(ordinal, _, _)| *ordinal);
            let parts: Vec<_> = ordered
                .iter()
                .map(|(_, message, part)| request.contents[*message].parts[*part].clone())
                .collect();
            for ((_, message, part), original) in positions.iter().zip(parts) {
                request.contents[*message].parts[*part] = original;
            }
        }
        self.cache.touch(restored.into_keys());
        Ok(())
    }
}

/// 数据库不可用与无效续接分别返回 503、422；容量不足在响应提交前返回 502。
async fn database<T>(
    operation: impl std::future::Future<Output = llmproxy_store::StoreResult<T>>,
    response: bool,
) -> pingora::Result<T> {
    let (status, reason) =
        match tokio::time::timeout(std::time::Duration::from_secs(5), operation).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(llmproxy_store::StoreError::NotFound)) => (422, "missing_or_expired"),
            Ok(Err(llmproxy_store::StoreError::Conflict(_))) if response => (502, "capacity"),
            Ok(Err(_)) => (503, "storage"),
            Err(_) => (504, "storage_timeout"),
        };
    let _ = failure(reason);
    Err(pingora::Error::explain(
        pingora::ErrorType::HTTPStatus(status),
        "tool continuation state unavailable",
    ))
}

/// 响应已完整编码后才发布引用；容量检查与提交位于同一个锁内。
pub struct Pending {
    cache: Arc<Cache>,
    entries: Vec<(String, Entry)>,
}
impl Pending {
    /// 编码成功后原子发布引用，未提交对象销毁时不会残留缓存项。
    pub fn commit(self) -> Result<()> {
        self.cache.commit(self.entries)
    }
}

/// Gemini 缺失参数等价于空对象，但恢复时仍保留来源的缺失形式。
fn normalized_args(
    args: &O<serde_json::Map<String, serde_json::Value>>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    match args {
        O::Missing => Some(Default::default()),
        O::Null => None,
        O::Value(args) => Some(args.clone()),
    }
}

/// 仅记录受控错误类别，不输出引用、签名、调用参数或作用域摘要。
fn failure(reason: &'static str) -> Error {
    tracing::warn!(
        component = "gateway",
        event_kind = "tool_state",
        reason,
        "tool continuation state unavailable"
    );
    Error::Unsupported(format!("tool continuation state: {reason}"))
}
