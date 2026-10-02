//! 在 Pingora 分块回调与 JSON 解析之间管理单次请求的正文边界。
//! 首版读取 JSON 文档和 SSE 事件，但保留原始字节。

mod model;
mod parse;
mod request;

pub use request::{ModelRead, RequestBody};

use bytes::Bytes;
use llmproxy_core::{
    adapter::{Error as AdapterError, protocol_codec::ProtocolCodec},
    protocol::Protocol,
};

// 限制单次请求的缓冲内存；尚无修改规则时，超限正文回退为原样转发。
const MAX_BUFFERED_BODY: usize = 8 * 1024 * 1024;

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
    // JSON 等待正文结束；SSE 只保留尚未完整的事件。
    pending: Vec<u8>,
}

impl BodyTransform {
    /// 创建指定正文类型的分块处理器。
    pub fn new(kind: BodyKind) -> Self {
        Self {
            kind,
            codec: None,
            pending: Vec::new(),
        }
    }

    /// 为 JSON 正文指定来源协议及请求或响应方向。
    pub fn set_codec(&mut self, protocol: Protocol, phase: MessagePhase) {
        self.codec = Some((protocol, phase));
    }

    /// 根据上游响应头切换正文处理方式，并清空上一种方式的暂存片段。
    pub fn replace_kind(&mut self, kind: BodyKind) {
        // 根据本次上游响应头重新选择正文处理方式。
        self.kind = kind;
        self.pending.clear();
    }

    /// 缓冲 JSON 到正文结束，或逐个放行完整 SSE 事件；超限后原样透传。
    pub fn push(&mut self, body: &mut Option<Bytes>, end: bool) {
        if matches!(self.kind, BodyKind::Passthrough) {
            return;
        }
        let chunk = body.take().unwrap_or_default();
        if self.pending.len().saturating_add(chunk.len()) > MAX_BUFFERED_BODY {
            // 先发出此前暂存的内容，后续分块直接透传，避免丢失字节。
            self.kind = BodyKind::Passthrough;
            self.pending.extend_from_slice(&chunk);
            *body = Some(Bytes::from(std::mem::take(&mut self.pending)));
            return;
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
            return;
        };
        // 发出完整事件，留下后续尚未完整的 SSE 事件。
        let remaining = self.pending.split_off(ready);
        let complete = std::mem::replace(&mut self.pending, remaining);
        let complete = Bytes::from(complete);
        *body = Some(match self.kind {
            BodyKind::Json => match parse::JsonDocument::decode(complete) {
                Ok(json) => process_json(json, self.codec),
                Err(original) => original,
            },
            BodyKind::Sse => {
                parse::sse_json(&complete, |_| {});
                complete
            }
            BodyKind::Passthrough => unreachable!(),
        });
    }
}

/// 在完整 JSON 中执行整体 IR 投影；当前未编辑 IR，保持原始字节输出。
fn process_json(mut json: parse::JsonDocument, codec: Option<(Protocol, MessagePhase)>) -> Bytes {
    if let Some((protocol, phase)) = codec {
        // 尚无 IR 编辑规则；解析失败时保持原文转发。
        let _ = json.apply::<AdapterError>(|body| {
            let encoded = match phase {
                MessagePhase::Request => {
                    let request = protocol.decode_request(body)?;
                    protocol.encode_request(&request)?
                }
                MessagePhase::Response => {
                    let response = protocol.decode_response(body)?;
                    protocol.encode_response(&response)?
                }
            };
            if encoded == *body {
                Ok(false)
            } else {
                *body = encoded;
                Ok(true)
            }
        });
    }
    json.encode()
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
            transform.push(&mut body, true);
            assert_eq!(body.unwrap().as_ref(), source);
        }
    }

    #[test]
    fn json_waits_for_end_and_preserves_original_bytes() {
        let mut transform = BodyTransform::new(BodyKind::Json);
        let mut first = Some(Bytes::from_static(br#" {"model":"a","input":"#));
        transform.push(&mut first, false);
        assert_eq!(first.unwrap(), Bytes::new());
        let mut last = Some(Bytes::from_static(br#""hello"} "#));
        transform.push(&mut last, true);
        assert_eq!(
            last.unwrap(),
            Bytes::from_static(br#" {"model":"a","input":"hello"} "#)
        );
    }

    #[test]
    fn sse_waits_only_for_a_complete_event() {
        let mut transform = BodyTransform::new(BodyKind::Sse);
        let mut first = Some(Bytes::from_static(b"data: {\"x\":1}\n"));
        transform.push(&mut first, false);
        assert_eq!(first.unwrap(), Bytes::new());
        let mut second = Some(Bytes::from_static(b"\ndata: [DONE]\n\npartial"));
        transform.push(&mut second, false);
        assert_eq!(
            second.unwrap(),
            Bytes::from_static(b"data: {\"x\":1}\n\ndata: [DONE]\n\n")
        );
        let mut end = None;
        transform.push(&mut end, true);
        assert_eq!(end.unwrap(), Bytes::from_static(b"partial"));
    }
}
