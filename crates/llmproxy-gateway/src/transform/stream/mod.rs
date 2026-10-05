//! SSE 只在接入边界读写；内部保持来源 struct → 事件 IR → 目标 struct。
mod observe;
use bytes::Bytes;
use llmproxy_core::{
    adapter::protocol_codec::{
        ProtocolCodec, ResponseTarget,
        stream::{Decoder, Encoder},
    },
    ir::stream::{Event, Limits},
    protocol::{
        Protocol,
        stream::{self, sse},
    },
};
pub(super) use observe::Observer;
use pingora::{Error, ErrorType, Result};

pub struct Stream {
    source: Protocol,
    target: Protocol,
    framing: sse::Decoder,
    decoder: Decoder,
    encoder: Encoder,
    capture: Option<crate::tool_state::StreamCapture>,
    usage_recorded: bool,
    sequence: Option<u64>,
    span: tracing::Span,
    terminal: Option<Event>,
}
impl Stream {
    /// 每次响应独立创建分帧和协议状态；签名调用组额外受容量限制。
    pub fn new(
        source: Protocol,
        target: Protocol,
        shell: &ResponseTarget<'_>,
        context: Option<crate::tool_state::Context>,
    ) -> Result<Self> {
        let limits = Limits::default();
        Ok(Self {
            source,
            target,
            framing: sse::Decoder::new(super::MAX_BUFFERED_BODY),
            decoder: source.stream_decoder(limits),
            encoder: target
                .stream_encoder(source, shell, limits)
                .map_err(|_| invalid())?,
            capture: context.map(|context| crate::tool_state::StreamCapture::new(context, target)),
            usage_recorded: false,
            sequence: None,
            span: tracing::Span::current(),
            terminal: None,
        })
    }
    /// 字节交给分帧器；调用方每拿到一帧就 await 处理及发送，维持背压。
    pub fn frame(&mut self, byte: u8) -> Result<Option<sse::Frame>> {
        self.framing.push(byte).map_err(|_| invalid())
    }
    /// 单帧直接解码来源 struct，目标输出按帧及单次交付总量限制。
    pub async fn convert(&mut self, frame: &sse::Frame) -> Result<Vec<Bytes>> {
        let raw = sse::decode(self.source, frame).map_err(|_| invalid())?;
        self.convert_raw(&raw).await
    }
    /// 正常 HTTP EOF 才补协议传输结束；未闭合事件或候选不伪装成成功。
    pub async fn finish(&mut self) -> Result<Vec<Bytes>> {
        self.framing.finish().map_err(|_| invalid())?;
        let mut output = self.convert_raw(&stream::Event::End(self.source)).await?;
        let event = self.terminal.take().ok_or_else(invalid)?;
        let mut length = output.iter().map(Bytes::len).sum();
        self.emit(&event, &mut output, &mut length)?;
        Ok(output)
    }
    /// 先完成本帧状态校验和签名持久化，再返回可交付的目标字节。
    async fn convert_raw(&mut self, raw: &stream::Event) -> Result<Vec<Bytes>> {
        let events = self.decoder.push(raw).map_err(|_| invalid())?;
        if let Some(capture) = &mut self.capture {
            capture.observe(raw, &events).map_err(|_| invalid())?;
        }
        let mut output = Vec::new();
        let mut length = 0usize;
        for event in events {
            if matches!(event, Event::End(_)) {
                self.terminal = Some(event);
                continue;
            }
            if matches!(event, Event::Failure(_)) {
                return Err(invalid());
            }
            let (events, pending) = if let Some(capture) = &mut self.capture {
                capture.take(event).map_err(|_| invalid())?
            } else {
                (vec![event], None)
            };
            for event in events {
                self.emit(&event, &mut output, &mut length)?;
            }
            if let Some(pending) = pending {
                self.capture.as_ref().unwrap().persist(pending).await?;
            }
        }
        Ok(output)
    }
    /// 每次交付的序列化字节有独立上限，成功终止只由正常 HTTP EOF 触发。
    fn emit(&mut self, event: &Event, output: &mut Vec<Bytes>, length: &mut usize) -> Result<()> {
        let conversion = self.encoder.push(event).map_err(|_| invalid())?;
        super::cross::warn(conversion.warnings);
        for raw in conversion.body {
            if let stream::Event::Responses(event) = &raw
                && let llmproxy_core::protocol::responses::response::Event::Known(event) = &**event
            {
                self.sequence = Some(event.sequence_number());
            }
            let bytes = sse::encode(&raw, super::MAX_BUFFERED_BODY).map_err(|_| invalid())?;
            *length = length.saturating_add(bytes.len());
            if *length > super::MAX_BUFFERED_BODY {
                return Err(invalid());
            }
            if !bytes.is_empty() {
                output.push(bytes.into());
            }
        }
        Ok(())
    }
    /// 正常、失败和取消均只记录一份来源累计快照；缺失字段保持缺失。
    pub fn record_usage(&mut self) {
        if !self.usage_recorded {
            self.usage_recorded = true;
            if let Some(usage) = self.decoder.state().usage().snapshot() {
                self.span.in_scope(|| {
                    crate::observability::response_usage(self.source, self.target, usage)
                });
            }
        }
    }
    /// Responses 的错误序号延续已生成的目标事件，其他协议使用各自错误外壳。
    pub fn failure(&self, status: u16) -> Bytes {
        error(
            self.target,
            status,
            self.sequence.map_or(0, |n| n.saturating_add(1)),
        )
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.record_usage();
    }
}
/// 无效事件不能暴露原始协议字段值或被当作正常终止。
fn invalid() -> Box<Error> {
    Error::explain(ErrorType::HTTPStatus(502), "invalid upstream event stream").into_up()
}

/// 已发 HTTP 头后的受控错误，不包含 Provider 正文，也不追加成功结束标记。
fn error(protocol: Protocol, status: u16, sequence: u64) -> Bytes {
    if protocol == Protocol::OpenAiResponses {
        use llmproxy_core::protocol::{
            OptionalNullable as O,
            responses::response::event::{ErrorEvent, Event, KnownEvent},
        };
        let event = stream::Event::Responses(Box::new(Event::Known(Box::new(KnownEvent::Error(
            ErrorEvent {
                code: O::Value("upstream_error".into()),
                message: "Provider request failed".into(),
                param: O::Null,
                sequence_number: sequence,
                extra: Default::default(),
            },
        )))));
        return sse::encode(&event, super::MAX_BUFFERED_BODY)
            .expect("固定错误字段可以序列化")
            .into();
    }
    let mut bytes = Vec::new();
    if matches!(
        protocol,
        Protocol::AnthropicMessages | Protocol::OpenAiResponses
    ) {
        bytes.extend_from_slice(b"event: error\n");
    }
    bytes.extend_from_slice(b"data: ");
    bytes.extend_from_slice(&super::error::body(
        protocol,
        if status == 0 { 502 } else { status },
    ));
    bytes.extend_from_slice(b"\n\n");
    bytes.into()
}
