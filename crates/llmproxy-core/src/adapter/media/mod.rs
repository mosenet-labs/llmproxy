//! 四协议媒体叶子与 IR 的直接映射；不执行网络下载或跨 Provider 上传。
mod decode;
mod encode;
mod metadata;
pub(crate) use decode::decode;
pub(crate) use encode::encode;

/// 已识别媒体进入通用 IR，其余叶子维持原协议不透明表示。
pub(crate) fn part(
    raw: crate::ir::media::OriginalMedia,
) -> super::Result<crate::ir::message::Part> {
    use crate::{
        ir::{media::OriginalMedia as Raw, message::PartKind},
        protocol::Protocol,
    };
    let protocol = match &raw {
        Raw::Chat(_) => Protocol::OpenAiChat,
        Raw::Responses(_) => Protocol::OpenAiResponses,
        Raw::Messages(_) => Protocol::AnthropicMessages,
        Raw::Gemini(_) => Protocol::Gemini,
    };
    if let Some(media) = decode(&raw) {
        return Ok(super::wire::part(
            PartKind::Media(media),
            protocol,
            "media",
            metadata::extra(&raw)?,
        ));
    }
    match raw {
        Raw::Chat(v) => super::wire::opaque_value(protocol, &v),
        Raw::Responses(v) => super::wire::opaque_value(protocol, &v),
        Raw::Messages(v) => super::wire::opaque_value(protocol, &v),
        Raw::Gemini(v) => super::wire::opaque_value(protocol, &v),
    }
}
