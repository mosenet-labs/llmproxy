use std::ops::Range;

use bytes::Bytes;
use llmproxy_core::protocol::Protocol;
use pingora::{Error, ErrorType, Result, proxy::Session};

use super::{
    BodyKind, BodyTransform, MessagePhase,
    model::{MODEL_PREFIX_LIMIT, Scan, rewrite_model, scan_model},
};

struct ModelPrefix {
    bytes: Vec<u8>,
    range: Range<usize>,
}

pub enum ModelRead {
    Found(String),
    Rejected(u16),
}

pub struct RequestBody {
    model_prefix: Option<ModelPrefix>,
    replay_prefix: Option<Bytes>,
    original_prefix_len: usize,
    body_delta: isize,
    body: BodyTransform,
}

impl RequestBody {
    /// 创建请求正文处理器，默认按 JSON 缓冲。
    pub fn new() -> Self {
        Self {
            model_prefix: None,
            replay_prefix: None,
            original_prefix_len: 0,
            body_delta: 0,
            body: BodyTransform::new(BodyKind::Json),
        }
    }

    /// 在路由前预读顶层模型别名，并保存已读前缀供后续回放。
    pub async fn read_model(&mut self, session: &mut Session) -> Result<ModelRead> {
        // 上游选择早于正文过滤，因此只预读定位模型所需的前缀，并交给 Pingora 回放。
        session.enable_retry_buffering();
        let mut prefix = Vec::new();
        let (alias, range) = loop {
            let Some(chunk) = session.read_request_body().await? else {
                return Ok(ModelRead::Rejected(400));
            };
            prefix.extend_from_slice(&chunk);
            if prefix.len() > MODEL_PREFIX_LIMIT || session.retry_buffer_truncated() {
                return Ok(ModelRead::Rejected(413));
            }
            match scan_model(&prefix) {
                Scan::Found { alias, range } => break (alias, range),
                Scan::More if prefix.len() < MODEL_PREFIX_LIMIT => {}
                Scan::More => return Ok(ModelRead::Rejected(413)),
                Scan::Invalid | Scan::Missing => return Ok(ModelRead::Rejected(400)),
            }
        };
        if alias.is_empty() || session.get_retry_buffer().is_none() {
            return Ok(ModelRead::Rejected(400));
        }
        self.model_prefix = Some(ModelPrefix {
            bytes: prefix,
            range,
        });
        Ok(ModelRead::Found(alias))
    }

    /// 将已保存前缀中的模型别名替换为上游模型 ID。
    pub fn select_model(&mut self, upstream_model_id: &str) -> serde_json::Result<()> {
        let ModelPrefix { bytes, range } = self
            .model_prefix
            .take()
            .expect("read_model selected a model prefix");
        let (rewritten, delta) = rewrite_model(&bytes, range, upstream_model_id)?;
        self.body_delta = delta;
        self.original_prefix_len = bytes.len();
        self.replay_prefix = Some(rewritten);
        Ok(())
    }

    /// 返回模型字段改写引起的正文字节数变化。
    pub fn body_delta(&self) -> isize {
        self.body_delta
    }

    /// 为请求 JSON 正文选择对应协议的编解码器。
    pub fn set_protocol(&mut self, protocol: Protocol) {
        self.body.set_codec(protocol, MessagePhase::Request);
    }

    /// 将路由阶段预读的前缀回放到正文过滤器，再处理当前分块。
    pub fn push(&mut self, body: &mut Option<Bytes>, end: bool) -> Result<()> {
        if let Some(rewritten) = self.replay_prefix.take() {
            // 路由阶段读取的前缀在此回放，再与后续正文交给同一个缓冲状态。
            if body.as_ref().map(Bytes::len) != Some(self.original_prefix_len) {
                return Err(Error::explain(
                    ErrorType::InternalError,
                    "request body replay prefix mismatch",
                ));
            }
            *body = Some(rewritten);
        }
        self.body.push(body, end);
        Ok(())
    }
}
