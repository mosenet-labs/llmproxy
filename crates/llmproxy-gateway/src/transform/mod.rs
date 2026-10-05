//! 在 Pingora 分块回调与 JSON 解析之间管理单次请求的正文边界。
//! 同协议保留未修改的原始字节；跨协议等待完整 JSON 后转换。

mod codec;
mod cross;
mod error;
pub use error::respond as respond_error;
mod model;
mod parse;
mod request;

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

#[derive(Default)]
pub struct BodyTransform {
    kind: BodyKind,
    codec: Option<(Protocol, MessagePhase)>,
    cross: Option<CrossConversion>,
    cross_error: Option<(Protocol, u16)>,
    tool_state: Option<crate::tool_state::Context>,
    // JSON 等待正文结束；SSE 只保留尚未完整的事件。
    pending: Vec<u8>,
}

impl BodyTransform {
    /// 创建指定正文类型的分块处理器。
    pub fn new(kind: BodyKind) -> Self {
        Self {
            kind,
            codec: None,
            cross: None,
            cross_error: None,
            tool_state: None,
            pending: Vec::new(),
        }
    }

    /// 为 JSON 正文指定来源协议及请求或响应方向。
    pub fn set_codec(&mut self, protocol: Protocol, phase: MessagePhase) {
        self.codec = Some((protocol, phase));
    }

    /// 保存本次请求固定的工具回合作用域，不在正文回调中重新选路。
    pub fn set_tool_state(&mut self, context: crate::tool_state::Context) {
        self.tool_state = Some(context);
    }

    /// 启用请求跨协议转换；完整 JSON 会在正文结束时写成 Provider 协议。
    pub fn set_cross_request(&mut self, source: Protocol, target: Protocol, model: &str) {
        self.cross = Some(CrossConversion::Request {
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
        self.cross = Some(CrossConversion::Response {
            source,
            target,
            model: model.to_owned(),
            id: id.to_owned(),
            created,
        });
    }

    /// 上游失败时保留 HTTP 状态，改用客户端协议的通用错误外壳。
    pub fn set_cross_error(&mut self, protocol: Protocol, status: u16) {
        self.cross_error = Some((protocol, status));
    }

    /// 根据上游响应头切换正文处理方式，并清空上一种方式的暂存片段。
    pub fn replace_kind(&mut self, kind: BodyKind) {
        // 根据本次上游响应头重新选择正文处理方式。
        self.kind = kind;
        self.pending.clear();
    }

    /// 缓冲 JSON 到正文结束，或逐个放行完整 SSE 事件；超限后原样透传。
    pub fn push(&mut self, body: &mut Option<Bytes>, end: bool) -> Result<()> {
        if let Some((protocol, status)) = self.cross_error {
            *body = Some(if end {
                error::body(protocol, status)
            } else {
                Bytes::new()
            });
            return Ok(());
        }
        if matches!(self.kind, BodyKind::Passthrough) {
            return Ok(());
        }
        let chunk = body.take().unwrap_or_default();
        if self.pending.len().saturating_add(chunk.len()) > MAX_BUFFERED_BODY {
            if let Some(cross) = &self.cross {
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
            return Ok(());
        }
        self.pending.extend_from_slice(&chunk);
        let ready = match self.kind {
            BodyKind::Json => end.then_some(self.pending.len()),
            BodyKind::Sse => parse::complete_sse_prefix(&self.pending, end),
            BodyKind::Passthrough => unreachable!(),
        };
        let Some(ready) = ready else {
            // Pingora 的请求路径可能将 None 视为正文结束；空 Bytes 表示当前暂无输出。
            *body = Some(Bytes::new());
            return Ok(());
        };
        // 发出完整事件，留下后续尚未完整的 SSE 事件。
        let remaining = self.pending.split_off(ready);
        let complete = std::mem::replace(&mut self.pending, remaining);
        let complete = Bytes::from(complete);
        *body = Some(match self.kind {
            BodyKind::Json => {
                if let Some(cross) = &self.cross {
                    cross::convert(complete, cross, self.tool_state.as_ref())?
                } else {
                    process_json(complete, self.codec)
                }
            }
            BodyKind::Sse => {
                parse::sse_json(&complete, |_| {});
                complete
            }
            BodyKind::Passthrough => unreachable!(),
        });
        Ok(())
    }
}

/// 在完整 JSON 中执行整体 IR 投影；当前未编辑 IR，保持原始字节输出。
fn process_json(bytes: Bytes, codec: Option<(Protocol, MessagePhase)>) -> Bytes {
    let Some((protocol, phase)) = codec else {
        return bytes;
    };
    // 无编辑时按类型比较并保留原始字节；解析失败沿用同协议透传行为。
    let result = (|| -> std::result::Result<Option<Vec<u8>>, AdapterError> {
        match phase {
            MessagePhase::Request => {
                let body = codec::decode_request(protocol, &bytes)?;
                let ir = protocol.decode_request(&body)?;
                let encoded = protocol.encode_request(&ir)?;
                if body == encoded {
                    Ok(None)
                } else {
                    Ok(Some(codec::encode_request(&encoded)?))
                }
            }
            MessagePhase::Response => {
                let body = codec::decode_response(protocol, &bytes)?;
                let ir = protocol.decode_response(&body)?;
                if let Some(usage) = &ir.usage {
                    crate::observability::response_usage(protocol, protocol, usage);
                }
                let encoded = protocol.encode_response(&ir)?;
                if body == encoded {
                    Ok(None)
                } else {
                    Ok(Some(codec::encode_response(&encoded)?))
                }
            }
        }
    })();
    match result {
        Ok(Some(encoded)) => Bytes::from(encoded),
        _ => bytes,
    }
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
    fn sse_waits_only_for_a_complete_event() {
        let mut transform = BodyTransform::new(BodyKind::Sse);
        let mut first = Some(Bytes::from_static(b"data: {\"x\":1}\n"));
        transform.push(&mut first, false).unwrap();
        assert_eq!(first.unwrap(), Bytes::new());
        let mut second = Some(Bytes::from_static(b"\ndata: [DONE]\n\npartial"));
        transform.push(&mut second, false).unwrap();
        assert_eq!(
            second.unwrap(),
            Bytes::from_static(b"data: {\"x\":1}\n\ndata: [DONE]\n\n")
        );
        let mut end = None;
        transform.push(&mut end, true).unwrap();
        assert_eq!(end.unwrap(), Bytes::from_static(b"partial"));
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
    fn cross_protocol_stream_request_is_rejected() {
        let mut request = BodyTransform::new(BodyKind::Json);
        request.set_cross_request(
            Protocol::OpenAiChat,
            Protocol::AnthropicMessages,
            "claude-model",
        );
        let mut body = Some(Bytes::from_static(br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32,"stream":true}"#));
        assert!(request.push(&mut body, true).is_err());
    }
}
