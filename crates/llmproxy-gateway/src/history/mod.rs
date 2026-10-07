//! 来源用量观察与内部轮次关联；正文回调不执行数据库 I/O。
use llmproxy_core::{
    ir::{request::controls::Reasoning, usage::Usage},
    protocol::Protocol,
};
use llmproxy_store::{
    ProviderStore,
    chat_history::{ActualCall, UsageState},
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(crate) struct Sink(Arc<Mutex<Snapshot>>);
#[derive(Clone, Default)]
struct Snapshot {
    actual: Option<ActualCall>,
    usage: Option<Usage>,
    final_usage: bool,
    saved: bool,
}
impl Sink {
    /// 来源快照整体替换，累计计数不会按帧重复相加。
    fn capture(&self, usage: Option<&Usage>, model: Option<&str>, final_usage: bool) {
        let mut state = self.0.lock().expect("history usage mutex");
        if let Some(usage) = usage {
            state.usage = Some(usage.clone());
        }
        if let Some(actual) = &mut state.actual
            && let Some(model) = model
        {
            actual.reported_model = Some(model.into());
        }
        state.final_usage |= final_usage;
    }
    /// 整包响应的终态与用量，在目标协议编码之前采集。
    pub fn response(&self, response: &llmproxy_core::ir::response::Response) {
        self.capture(
            response.usage.as_ref(),
            response.model.as_deref(),
            terminal(Some(response.status)),
        );
    }
    /// 同协议观察器与跨协议流复用来源解码器的累计状态。
    pub fn stream(&self, state: &llmproxy_core::ir::stream::State, valid: bool) {
        self.capture(
            state.usage().snapshot(),
            state.metadata().and_then(|m| m.model.as_deref()),
            valid && terminal(state.ended()),
        );
    }
    /// 记录解码后实际应用的配置，默认请求保持未知而不推测。
    pub fn reasoning(&self, reasoning: &Reasoning) {
        if let Some(actual) = &mut self.0.lock().expect("history usage mutex").actual {
            actual.reasoning = Some(reasoning.clone());
        }
    }
}
#[derive(Clone)]
pub(crate) struct Tracker {
    key: String,
    store: ProviderStore,
    pub sink: Sink,
}
impl Tracker {
    /// 创建本轮来源采集器，只有已认证内部请求使用。
    pub fn new(key: String, store: ProviderStore) -> Self {
        Self {
            key,
            store,
            sink: Sink::default(),
        }
    }
    /// 已认证轮次只认领一次，验证来源模型与协议。
    pub async fn start(
        &self,
        alias: &str,
        protocol: Protocol,
        actual: ActualCall,
    ) -> pingora::Result<()> {
        self.store
            .start_chat_call(&self.key, alias, protocol, &actual)
            .await
            .map_err(|_| {
                pingora::Error::explain(
                    pingora::ErrorType::HTTPStatus(503),
                    "history request could not be saved",
                )
            })?;
        self.sink.0.lock().expect("history usage mutex").actual = Some(actual);
        Ok(())
    }
    /// 异步结束时保存现有来源快照，取消不会把未报告用量补为零。
    pub async fn finish(&self) -> Result<(), llmproxy_store::StoreError> {
        let state = self.sink.0.lock().expect("history usage mutex").clone();
        if state.saved {
            return Ok(());
        }
        let Some(actual) = state.actual else {
            return Ok(());
        };
        let quality = if state.usage.is_none() {
            UsageState::Unreported
        } else if state.final_usage {
            UsageState::Final
        } else {
            UsageState::Partial
        };
        self.store
            .save_chat_usage(&self.key, &actual, state.usage.as_ref(), quality)
            .await?;
        self.sink.0.lock().expect("history usage mutex").saved = true;
        Ok(())
    }
}

/// 成功、达到上限及 Provider 主动失败／取消均是协议终态。
fn terminal(status: Option<llmproxy_core::ir::response::Status>) -> bool {
    use llmproxy_core::ir::response::Status;
    matches!(
        status,
        Some(Status::Completed | Status::Incomplete | Status::Failed | Status::Cancelled)
    )
}
