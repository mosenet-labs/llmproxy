//! 同协议只观察统计，不修改原始 SSE；未知或无效事件仍按原始字节转发。
use llmproxy_core::{
    adapter::protocol_codec::{ProtocolCodec, stream::Decoder},
    ir::stream::Limits,
    protocol::{
        Protocol,
        stream::{self, sse},
    },
};

pub(in crate::transform) struct Observer {
    protocol: Protocol,
    framing: sse::Decoder,
    decoder: Decoder,
    active: bool,
    recorded: bool,
    span: tracing::Span,
}
impl Observer {
    /// 独立观察状态只用于统计，生命周期校验失败不会影响同协议透传。
    pub(in crate::transform) fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            framing: sse::Decoder::new(super::super::MAX_BUFFERED_BODY),
            decoder: protocol.stream_decoder(Limits::default()),
            active: true,
            recorded: false,
            span: tracing::Span::current(),
        }
    }
    /// 逐帧读取来源计数；无法识别的流停止观察，仍保留此前已报告的用量。
    pub(in crate::transform) fn push(&mut self, bytes: &[u8], end: bool) {
        if self.active && self.read(bytes, end).is_err() {
            self.active = false;
        }
        if end {
            self.record();
        }
    }
    /// 只解析完整事件，HTTP EOF 之后检查来源协议的正常终止。
    fn read(&mut self, bytes: &[u8], end: bool) -> Result<(), ()> {
        for byte in bytes {
            if let Some(frame) = self.framing.push(*byte).map_err(|_| ())? {
                let raw = sse::decode(self.protocol, &frame).map_err(|_| ())?;
                self.decoder.push(&raw).map_err(|_| ())?;
            }
        }
        if end {
            self.framing.finish().map_err(|_| ())?;
            self.decoder
                .push(&stream::Event::End(self.protocol))
                .map_err(|_| ())?;
        }
        Ok(())
    }
    /// 正常结束或观察器销毁时仅记录一次实际累计快照。
    fn record(&mut self) {
        if !self.recorded {
            self.recorded = true;
            if let Some(usage) = self.decoder.state().usage().snapshot() {
                self.span.in_scope(|| {
                    crate::observability::response_usage(self.protocol, self.protocol, usage)
                });
            }
        }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.record();
    }
}
