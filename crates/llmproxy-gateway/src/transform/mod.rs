//! 在 Pingora 分块回调与 JSON 解析之间管理单次请求的正文边界。
//! 首版读取 JSON 文档和 SSE 事件，但保留原始字节。

mod model;
mod parse;
mod request;

pub use request::{ModelRead, RequestBody};

use bytes::Bytes;

// 限制单次请求的缓冲内存；尚无修改规则时，超限正文回退为原样转发。
const MAX_BUFFERED_BODY: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub enum BodyKind {
    #[default]
    Passthrough,
    Json,
    Sse,
}

#[derive(Default)]
pub struct BodyTransform {
    kind: BodyKind,
    // JSON 等待正文结束；SSE 只保留尚未完整的事件。
    pending: Vec<u8>,
}

impl BodyTransform {
    pub fn new(kind: BodyKind) -> Self {
        Self {
            kind,
            pending: Vec::new(),
        }
    }

    pub fn replace_kind(&mut self, kind: BodyKind) {
        // 根据本次上游响应头重新选择正文处理方式。
        self.kind = kind;
        self.pending.clear();
    }

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
                Ok(json) => process_json(json),
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

fn process_json(json: parse::JsonDocument) -> Bytes {
    // 首版没有处理规则；后续可在编码前修改解码后的值。
    let _ = json.value();
    json.encode()
}

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
    use super::{BodyKind, BodyTransform};
    use bytes::Bytes;

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
