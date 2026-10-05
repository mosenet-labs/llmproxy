//! 四种协议的请求／响应结构体与 IR 双向转换；JSON 处理由接入边界负责。

mod cross;
mod mode;
mod projection;
mod request;
mod response;
mod roundtrip;
pub mod stream;

pub use cross::{Conversion, ConversionWarning, RequestTarget, ResponseTarget};
pub use mode::EncodeMode;

#[cfg(test)]
mod cross_tests;
#[cfg(test)]
mod json_test_support;
#[cfg(test)]
mod normalized_tests;
#[cfg(test)]
mod stream_request_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod typed_tests;

use crate::protocol::{Request as ProtocolRequest, Response as ProtocolResponse};

use crate::{
    ir::{request::Request, response::Response},
    protocol::Protocol,
};

use super::{Error, Result};

/// 请求、非流式响应及流式事件状态的协议编解码契约。
/// 整包编码显式选择保留或重建模式；流式按事件规则构造目标载体。
pub trait ProtocolCodec {
    /// 为单次响应创建事件解码状态；与非流式转换共用同一协议入口。
    fn stream_decoder(&self, limits: crate::ir::stream::Limits) -> stream::Decoder;
    /// 为单次响应创建目标事件编码器；返回类型化事件与可记录的降级警告。
    fn stream_encoder(
        &self,
        source: Protocol,
        target: &ResponseTarget<'_>,
        limits: crate::ir::stream::Limits,
    ) -> Result<stream::Encoder>;
    /// 将来源协议请求类型投影为 IR。
    fn decode_request(&self, body: &ProtocolRequest) -> Result<Request>;
    /// 返回目标请求和转换警告；Preserve 使用来源外壳，Rebuild 使用 target。
    fn encode_request(
        &self,
        request: &Request,
        target: &RequestTarget<'_>,
        mode: EncodeMode,
    ) -> Result<Conversion<ProtocolRequest>>;
    /// 将来源协议非流式响应类型投影为 IR。
    fn decode_response(&self, body: &ProtocolResponse) -> Result<Response>;
    /// 返回目标响应和转换警告；保留与重建均通过显式模式选择。
    fn encode_response(
        &self,
        response: &Response,
        target: &ResponseTarget<'_>,
        mode: EncodeMode,
    ) -> Result<Conversion<ProtocolResponse>>;
}

impl ProtocolCodec for Protocol {
    fn stream_encoder(
        &self,
        source: Protocol,
        target: &ResponseTarget<'_>,
        limits: crate::ir::stream::Limits,
    ) -> Result<stream::Encoder> {
        stream::Encoder::new(*self, source, target, limits)
    }
    fn stream_decoder(&self, limits: crate::ir::stream::Limits) -> stream::Decoder {
        stream::Decoder::new(*self, limits)
    }
    fn decode_request(&self, body: &ProtocolRequest) -> Result<Request> {
        if body.protocol() != *self {
            return Err(Error::Invalid("协议类型与编解码器不匹配".into()));
        }
        let mut request = project_request(body)?;
        request.source = Some(body.clone());
        Ok(request)
    }

    fn encode_request(
        &self,
        request: &Request,
        target: &RequestTarget<'_>,
        mode: EncodeMode,
    ) -> Result<Conversion<ProtocolRequest>> {
        match mode {
            EncodeMode::Preserve => {
                roundtrip::encode_request(*self, request).map(Conversion::exact)
            }
            EncodeMode::Rebuild => cross::encode_request(*self, request, target),
        }
    }

    fn decode_response(&self, body: &ProtocolResponse) -> Result<Response> {
        if body.protocol() != *self {
            return Err(Error::Invalid("协议类型与编解码器不匹配".into()));
        }
        let mut response = project_response(body)?;
        response.source = Some(body.clone());
        Ok(response)
    }

    fn encode_response(
        &self,
        response: &Response,
        target: &ResponseTarget<'_>,
        mode: EncodeMode,
    ) -> Result<Conversion<ProtocolResponse>> {
        match mode {
            EncodeMode::Preserve => {
                roundtrip::encode_response(*self, response).map(Conversion::exact)
            }
            EncodeMode::Rebuild => cross::encode_response(*self, response, target),
        }
    }
}

/// 来源基线与回写校验共用纯投影，不为比较额外保存整包副本。
fn project_request(source: &ProtocolRequest) -> Result<Request> {
    let messages = request::decode_messages(source)?;
    let mut request = Request {
        items: request::decode_items(source, messages.len())?,
        instructions: request::decode_instructions(source),
        messages,
        cache: request::decode_cache(source),
        ..Request::new(source.protocol())
    };
    projection::decode_request(&mut request, source);
    Ok(request)
}

/// 响应校验同样只生成规范化字段，不克隆来源响应外壳。
fn project_response(source: &ProtocolResponse) -> Result<Response> {
    let messages = response::decode_messages(source)?;
    let mut response = Response {
        items: response::decode_items(source, messages.len())?,
        messages,
        usage: response::decode_usage(source),
        ..Response::new(source.protocol())
    };
    projection::decode_response(&mut response, source);
    Ok(response)
}

/// 只替换编辑过的消息，保留未改动消息的协议类型与扩展字段。
fn encode_changed<D: Clone, M: PartialEq>(
    original: &[D],
    edited: &[M],
    before: &[M],
    encode: impl FnOnce(&[M]) -> Result<Vec<D>>,
) -> Result<Option<Vec<D>>> {
    if edited.len() != original.len() || before.len() != original.len() {
        return Err(Error::Unsupported("当前不支持改变消息数量".into()));
    }
    if edited == before {
        return Ok(None);
    }
    let encoded = encode(edited)?;
    if encoded.len() != original.len() {
        return Err(Error::Invalid("消息适配器改变了消息数量".into()));
    }
    Ok(Some(
        original
            .iter()
            .zip(encoded)
            .zip(before.iter().zip(edited))
            .map(
                |((old, new), (before, after))| {
                    if before == after { old.clone() } else { new }
                },
            )
            .collect(),
    ))
}

#[cfg(test)]
mod nonstream_tests;

#[cfg(test)]
mod boundary_tests;
#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod native_tests;
