//! 在 Pingora 分块回调与 JSON 解析之间管理单次请求的正文边界。
//! 同协议保留未修改的原始字节；跨协议等待完整 JSON 后转换。

use llmproxy_core::protocol::wire as codec;
mod cross;
mod error;
pub use error::respond as respond_error;
pub use error::respond_message as respond_error_message;
mod model;
mod request;
#[cfg(test)]
mod request_tests;
pub mod stream;

pub use request::{ModelRead, RequestBody};

use bytes::Bytes;
use llmproxy_core::{
    adapter::{Error as AdapterError, protocol_codec::ProtocolCodec},
    protocol::Protocol,
};
use pingora::{Error, ErrorType, Result};

use self::cross::CrossConversion;

// 限制单次正文的缓冲内存；同协议超限时回退原样转发，跨协议则报错。
pub(crate) const MAX_BUFFERED_BODY: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub enum BodyKind {
    #[default]
    Passthrough,
    Json,
    Sse,
}

#[derive(Clone, Copy)]
pub enum MessagePhase {
    Request,
    Response,
}

/// 同一次正文只能观察、转换或替换错误，避免多个可选标志同时生效。
enum Operation {
    Observe(Option<(Protocol, MessagePhase)>),
    Convert(CrossConversion),
    Error { protocol: Protocol, status: u16 },
}
impl Default for Operation {
    fn default() -> Self {
        Self::Observe(None)
    }
}

#[derive(Default)]
pub struct BodyTransform {
    kind: BodyKind,
    operation: Operation,
    tool_state: Option<crate::tool_state::Context>,
    // 仅 JSON 缓冲整包；同协议 SSE 原样交付，观察器独立分帧。
    pending: Vec<u8>,
    observer: Option<stream::Observer>,
}

impl BodyTransform {
    /// 创建指定正文类型的分块处理器。
    pub fn new(kind: BodyKind) -> Self {
        Self {
            kind,
            operation: Operation::default(),
            tool_state: None,
            pending: Vec::new(),
            observer: None,
        }
    }

    /// 为 JSON 正文指定来源协议及请求或响应方向。
    pub fn set_codec(&mut self, protocol: Protocol, phase: MessagePhase) {
        if matches!(self.operation, Operation::Observe(_)) {
            self.operation = Operation::Observe(Some((protocol, phase)));
            self.observer = (matches!(phase, MessagePhase::Response)
                && matches!(self.kind, BodyKind::Sse))
            .then(|| stream::Observer::new(protocol));
        }
    }

    /// 请求准备阶段只读取转换状态，不依赖可选标志的组合。
    fn conversion(&self) -> Option<&CrossConversion> {
        match &self.operation {
            Operation::Convert(conversion) => Some(conversion),
            _ => None,
        }
    }

    /// 保存本次请求固定的工具回合作用域，不在正文回调中重新选路。
    pub fn set_tool_state(&mut self, context: crate::tool_state::Context) {
        self.tool_state = Some(context);
    }

    /// 启用请求跨协议转换；完整 JSON 会在正文结束时写成 Provider 协议。
    pub fn set_cross_request(&mut self, source: Protocol, target: Protocol, model: &str) {
        self.observer = None;
        self.operation = Operation::Convert(CrossConversion::Request {
            source,
            target,
            model: model.to_owned(),
        });
    }

    /// 启用响应跨协议转换；外壳由 Gateway 的客户端路由上下文提供。
    pub fn set_cross_response(
        &mut self,
        source: Protocol,
        target: Protocol,
        model: &str,
        id: &str,
        created: i64,
    ) {
        self.observer = None;
        self.operation = Operation::Convert(CrossConversion::Response {
            source,
            target,
            model: model.to_owned(),
            id: id.to_owned(),
            created,
        });
    }

    /// 上游失败时保留 HTTP 状态，改用客户端协议的通用错误外壳。
    pub fn set_cross_error(&mut self, protocol: Protocol, status: u16) {
        self.observer = None;
        self.pending.clear();
        self.operation = Operation::Error { protocol, status };
    }

    /// 根据上游响应头切换正文处理方式，并清空上一种方式的暂存片段。
    pub fn replace_kind(&mut self, kind: BodyKind) {
        // 根据本次上游响应头重新选择正文处理方式。
        self.kind = kind;
        self.pending.clear();
    }

    /// 缓冲 JSON 到正文结束；同协议 SSE 原样透传并由观察器读取用量。
    pub fn push(&mut self, body: &mut Option<Bytes>, end: bool) -> Result<()> {
        if let Some(observer) = &mut self.observer {
            observer.push(body.as_deref().unwrap_or_default(), end);
        }
        if let Some(complete) = self.collect(body, end)? {
            *body = Some(self.process(complete)?);
        }
        Ok(())
    }

    /// 请求正文完成后异步恢复工具状态，类型化请求只解析和转换一次。
    pub async fn push_request(&mut self, body: &mut Option<Bytes>, end: bool) -> Result<()> {
        if let Some(complete) = self.collect(body, end)? {
            *body = Some(
                if let Some(cross @ CrossConversion::Request { .. }) = self.conversion() {
                    cross::convert_request(complete, cross, self.tool_state.as_ref()).await?
                } else {
                    self.process(complete)?
                },
            );
        }
        Ok(())
    }

    /// 编码后的新工具引用先写入共享存储，父请求随后才提交客户端响应。
    pub async fn persist_tool_state(&self) -> Result<()> {
        if let Some(state) = &self.tool_state {
            state.persist_response().await?;
        }
        Ok(())
    }

    /// 流式父请求与子请求复用同一签名作用域，父请求负责异步提交。
    pub fn tool_context(&self) -> Option<crate::tool_state::Context> {
        self.tool_state.clone()
    }

    /// 只管理正文边界，返回完整载荷；协议转换及数据库 I/O 由调用阶段负责。
    fn collect(&mut self, body: &mut Option<Bytes>, end: bool) -> Result<Option<Bytes>> {
        if let Operation::Error { protocol, status } = self.operation {
            *body = Some(if end {
                error::body(protocol, status)
            } else {
                Bytes::new()
            });
            return Ok(None);
        }
        if matches!(self.kind, BodyKind::Passthrough | BodyKind::Sse) {
            return Ok(None);
        }
        let chunk = body.take().unwrap_or_default();
        if self.pending.len().saturating_add(chunk.len()) > MAX_BUFFERED_BODY {
            if let Some(cross) = self.conversion() {
                let status = if matches!(cross, CrossConversion::Response { .. }) {
                    502
                } else {
                    413
                };
                return Err(Error::explain(
                    ErrorType::HTTPStatus(status),
                    "cross-protocol JSON body exceeds limit",
                ));
            }
            // 先发出此前暂存的内容，后续分块直接透传，避免丢失字节。
            self.kind = BodyKind::Passthrough;
            self.pending.extend_from_slice(&chunk);
            *body = Some(Bytes::from(std::mem::take(&mut self.pending)));
            return Ok(None);
        }
        self.pending.extend_from_slice(&chunk);
        if !end {
            // Pingora 的请求路径可能将 None 视为正文结束；空 Bytes 表示当前暂无输出。
            *body = Some(Bytes::new());
            return Ok(None);
        }
        // JSON 完整后取出缓冲，字节所有权直接交给输出。
        Ok(Some(Bytes::from(std::mem::take(&mut self.pending))))
    }

    /// 同步响应转换只暂存新引用，持久化留给尚未发头的父请求。
    fn process(&self, complete: Bytes) -> Result<Bytes> {
        match &self.operation {
            Operation::Convert(cross) => cross::convert(complete, cross, self.tool_state.as_ref()),
            Operation::Observe(codec) => Ok(process_json(complete, *codec)),
            Operation::Error { .. } => unreachable!("错误状态已在正文收集阶段处理"),
        }
    }
}

/// 完整 JSON 只投影和观察 IR；未编辑时直接保留原始字节输出。
fn process_json(bytes: Bytes, codec: Option<(Protocol, MessagePhase)>) -> Bytes {
    let Some((protocol, phase)) = codec else {
        return bytes;
    };
    // 当前分支仅观察 IR，完整转换由跨协议分支承担；保持输入字节和同协议透传策略。
    let _ = (|| -> std::result::Result<(), AdapterError> {
        match phase {
            MessagePhase::Request => {
                let body = codec::decode_request(protocol, &bytes)?;
                protocol.decode_request(&body)?;
            }
            MessagePhase::Response => {
                let body = codec::decode_response(protocol, &bytes)?;
                let ir = protocol.decode_response(&body)?;
                if let Some(usage) = &ir.usage {
                    crate::observability::response_usage(protocol, protocol, usage);
                }
            }
        }
        Ok(())
    })();
    bytes
}

/// 仅对未压缩的 JSON 和 SSE 响应启用相应的正文处理方式。
pub fn response_kind(content_type: Option<&[u8]>, content_encoding: Option<&[u8]>) -> BodyKind {
    // 以下解析器只处理未压缩的 JSON 或 SSE 字节。
    if content_encoding.is_some_and(|value| !value.eq_ignore_ascii_case(b"identity")) {
        return BodyKind::Passthrough;
    }
    let Some(content_type) = content_type else {
        return BodyKind::Passthrough;
    };
    let media_type = content_type
        .split(|byte| *byte == b';')
        .next()
        .unwrap_or_default();
    let media_type = media_type.trim_ascii();
    if media_type.eq_ignore_ascii_case(b"text/event-stream") {
        BodyKind::Sse
    } else if media_type.eq_ignore_ascii_case(b"application/json")
        || media_type
            .get(media_type.len().saturating_sub(5)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b"+json"))
    {
        BodyKind::Json
    } else {
        BodyKind::Passthrough
    }
}

#[cfg(test)]
mod tests {
    use super::{BodyKind, BodyTransform, MessagePhase};
    use bytes::Bytes;
    use llmproxy_core::protocol::Protocol;

    #[test]
    fn full_ir_round_trip_keeps_unedited_json_bytes() {
        let cases = [
            (
                MessagePhase::Request,
                br#" { "model":"m", "messages":[{"role":"user","content":"hi"}], "prompt_cache_key":"key" } "#
                    .as_slice(),
            ),
            (
                MessagePhase::Response,
                br#" { "id":"c1", "created":1, "model":"m", "object":"chat.completion", "choices":[{"finish_reason":"stop","index":0,"message":{"role":"assistant","content":"hi"}}], "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":3}} } "#
                    .as_slice(),
            ),
        ];
        for (phase, source) in cases {
            let mut transform = BodyTransform::new(BodyKind::Json);
            transform.set_codec(Protocol::OpenAiChat, phase);
            let mut body = Some(Bytes::copy_from_slice(source));
            transform.push(&mut body, true).unwrap();
            assert_eq!(body.unwrap().as_ref(), source);
        }
    }

    #[test]
    fn json_waits_for_end_and_preserves_original_bytes() {
        let mut transform = BodyTransform::new(BodyKind::Json);
        let mut first = Some(Bytes::from_static(br#" {"model":"a","input":"#));
        transform.push(&mut first, false).unwrap();
        assert_eq!(first.unwrap(), Bytes::new());
        let mut last = Some(Bytes::from_static(br#""hello"} "#));
        transform.push(&mut last, true).unwrap();
        assert_eq!(
            last.unwrap(),
            Bytes::from_static(br#" {"model":"a","input":"hello"} "#)
        );
    }

    #[test]
    fn same_protocol_sse_preserves_original_http_chunks() {
        let mut transform = BodyTransform::new(BodyKind::Sse);
        transform.set_codec(Protocol::OpenAiChat, MessagePhase::Response);
        for chunk in [
            b"data: {\"x\":1}\r".as_slice(),
            b"\n\r\ndata: [DONE]\n\npartial",
        ] {
            let original = Bytes::copy_from_slice(chunk);
            let mut body = Some(original.clone());
            transform.push(&mut body, false).unwrap();
            assert_eq!(body, Some(original));
        }
        let mut end = None;
        transform.push(&mut end, true).unwrap();
        assert!(end.is_none());
    }

    #[test]
    fn error_state_replaces_conversion_and_is_not_reset_by_observation() {
        let mut transform = BodyTransform::new(BodyKind::Json);
        transform.set_cross_response(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            "m",
            "id",
            1,
        );
        let mut body = Some(Bytes::from_static(b"private provider prefix"));
        transform.push(&mut body, false).unwrap();
        transform.set_cross_error(Protocol::OpenAiChat, 429);
        transform.set_codec(Protocol::OpenAiChat, MessagePhase::Response);
        transform.replace_kind(BodyKind::Sse);
        let mut body = Some(Bytes::from_static(b"private provider tail"));
        transform.push(&mut body, false).unwrap();
        assert!(body.unwrap().is_empty());
        let mut body = None;
        transform.push(&mut body, true).unwrap();
        let body = body.unwrap();
        assert_eq!(body, super::error::body(Protocol::OpenAiChat, 429));
        assert!(!String::from_utf8_lossy(&body).contains("private"));
    }

    #[test]
    fn cross_protocol_json_waits_for_full_request_and_response() {
        let mut request = BodyTransform::new(BodyKind::Json);
        request.set_cross_request(
            Protocol::OpenAiChat,
            Protocol::AnthropicMessages,
            "claude-model",
        );
        let mut first = Some(Bytes::from_static(br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32"#));
        request.push(&mut first, false).unwrap();
        assert!(first.unwrap().is_empty());
        let mut last = Some(Bytes::from_static(b"}"));
        request.push(&mut last, true).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&last.unwrap()).unwrap();
        assert_eq!(body["model"], "claude-model");
        assert_eq!(body["max_tokens"], 32);
        assert_eq!(body["messages"][0]["content"], "hi");

        let mut response = BodyTransform::new(BodyKind::Json);
        response.set_cross_response(
            Protocol::AnthropicMessages,
            Protocol::OpenAiChat,
            "public-model",
            "llmproxy-test",
            42,
        );
        let mut body = Some(Bytes::from_static(br#"{"type":"message","id":"msg_1","model":"claude-model","role":"assistant","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":2}}"#));
        response.push(&mut body, true).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body.unwrap()).unwrap();
        assert_eq!(body["model"], "public-model");
        assert_eq!(body["id"], "llmproxy-test");
        assert_eq!(body["choices"][0]["message"]["content"], "hello");
        assert_eq!(body["usage"]["prompt_tokens"], 3);
    }

    #[test]
    fn cross_protocol_request_preserves_stream_intent() {
        let mut request = BodyTransform::new(BodyKind::Json);
        request.set_cross_request(
            Protocol::OpenAiChat,
            Protocol::AnthropicMessages,
            "claude-model",
        );
        let mut body = Some(Bytes::from_static(br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32,"stream":true}"#));
        request.push(&mut body, true).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body.unwrap()).unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "claude-model");
    }
}
