//! 四种协议的请求／响应结构体与 IR 双向转换；JSON 处理由接入边界负责。

mod cross;
mod projection;
mod request;
mod response;
mod roundtrip;

pub use cross::{Conversion, ConversionWarning, RequestTarget, ResponseTarget};

#[cfg(test)]
mod cross_tests;
#[cfg(test)]
mod json_test_support;
#[cfg(test)]
mod normalized_tests;
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

/// 请求及非流式响应的协议编解码契约；同协议可保留尚未规范化的字段。
pub trait ProtocolCodec {
    /// 将来源协议请求类型投影为 IR。
    fn decode_request(&self, body: &ProtocolRequest) -> Result<Request>;
    /// 编码为当前协议：同协议保留来源字段，否则从 IR 构造正文。
    /// 跨协议调用若需处理有损警告，应使用 `encode_request_for`。
    fn encode_request(&self, request: &Request) -> Result<ProtocolRequest>;
    /// 将来源协议非流式响应类型投影为 IR。
    fn decode_response(&self, body: &ProtocolResponse) -> Result<Response>;
    /// 编码为当前协议的非流式响应；无来源副本时使用 IR 中的响应外壳。
    /// 跨协议调用若需处理有损警告，应使用 `encode_response_for`。
    fn encode_response(&self, response: &Response) -> Result<ProtocolResponse>;
    /// 将 IR 编为指定目标协议的非流式请求，返回无法保留的语义警告。
    fn encode_request_for(
        &self,
        request: &Request,
        target: &RequestTarget<'_>,
    ) -> Result<Conversion<ProtocolRequest>>;
    /// 将 IR 编为指定目标协议的非流式响应；响应外壳由调用方提供。
    fn encode_response_for(
        &self,
        response: &Response,
        target: &ResponseTarget<'_>,
    ) -> Result<Conversion<ProtocolResponse>>;
}

impl ProtocolCodec for Protocol {
    fn encode_request_for(
        &self,
        request: &Request,
        target: &RequestTarget<'_>,
    ) -> Result<Conversion<ProtocolRequest>> {
        if *self == request.source_protocol() && request.source.is_some() {
            return Ok(Conversion::exact(self.encode_request(request)?));
        }
        cross::encode_request(*self, request, target)
    }

    fn encode_response_for(
        &self,
        response: &Response,
        target: &ResponseTarget<'_>,
    ) -> Result<Conversion<ProtocolResponse>> {
        if *self == response.source_protocol() && response.source.is_some() {
            return Ok(Conversion::exact(self.encode_response(response)?));
        }
        cross::encode_response(*self, response, target)
    }
    fn decode_request(&self, body: &ProtocolRequest) -> Result<Request> {
        if body.protocol() != *self {
            return Err(Error::Invalid("协议类型与编解码器不匹配".into()));
        }
        let source = body.clone();
        let messages = request::decode_messages(&source)?;
        let mut request = Request {
            items: request::decode_items(&source, messages.len())?,
            instructions: request::decode_instructions(&source),
            messages,
            cache: request::decode_cache(&source),
            ..Request::new(source.protocol())
        };
        projection::decode_request(&mut request, &source);
        request.source = Some(source);
        Ok(request)
    }

    fn encode_request(&self, request: &Request) -> Result<ProtocolRequest> {
        let Some(original) = request
            .source
            .as_ref()
            .filter(|_| *self == request.source_protocol())
        else {
            return Ok(cross::encode_request(
                *self,
                request,
                &RequestTarget {
                    model: request.model.as_deref().unwrap_or(""),
                    max_output_tokens: None,
                },
            )?
            .body);
        };
        if request.instructions != request::decode_instructions(original)
            || request.items != request::decode_items(original, request.messages.len())?
        {
            return Err(Error::Unsupported(
                "同协议回写暂不支持修改顶层指令或独立输入项".into(),
            ));
        }
        let mut source = original.clone();
        request::encode_messages(&mut source, &request.messages)?;
        if request.cache != request::decode_cache(original) {
            request::encode_cache(&mut source, &request.cache);
            if request::decode_cache(&source) != request.cache {
                return Err(Error::Unsupported(
                    "目标协议无法表达修改后的缓存设置".into(),
                ));
            }
        }
        roundtrip::request(*self, request, original, &mut source)?;
        Ok(source)
    }

    fn decode_response(&self, body: &ProtocolResponse) -> Result<Response> {
        if body.protocol() != *self {
            return Err(Error::Invalid("协议类型与编解码器不匹配".into()));
        }
        let source = body.clone();
        let messages = response::decode_messages(&source)?;
        let mut response = Response {
            items: response::decode_items(&source, messages.len())?,
            messages,
            usage: response::decode_usage(&source),
            ..Response::new(source.protocol())
        };
        projection::decode_response(&mut response, &source);
        response.source = Some(source);
        Ok(response)
    }

    fn encode_response(&self, response: &Response) -> Result<ProtocolResponse> {
        let Some(original) = response
            .source
            .as_ref()
            .filter(|_| *self == response.source_protocol())
        else {
            let created = match response.created_at {
                Some(created) => created,
                None if matches!(self, Protocol::Gemini | Protocol::AnthropicMessages) => 0,
                None => return Err(Error::Unsupported("目标响应缺少创建时间".into())),
            };
            return Ok(cross::encode_response(
                *self,
                response,
                &ResponseTarget {
                    model: response.model.as_deref().unwrap_or(""),
                    id: response.id.as_deref().unwrap_or(""),
                    created,
                },
            )?
            .body);
        };
        if response.items != response::decode_items(original, response.messages.len())? {
            return Err(Error::Unsupported(
                "同协议回写暂不支持修改独立输出项".into(),
            ));
        }
        let mut source = original.clone();
        response::encode_messages(&mut source, &response.messages)?;
        let original_usage = response::decode_usage(original);
        if response.usage != original_usage {
            response::encode_usage(&mut source, response.usage.as_ref())?;
            if !response::changed_usage_fields_match(
                &original_usage,
                &response.usage,
                &response::decode_usage(&source),
            ) {
                return Err(Error::Unsupported("目标协议无法表达修改后的用量".into()));
            }
        }
        roundtrip::response(*self, response, original, &mut source)?;
        Ok(source)
    }
}

/// 只替换编辑过的消息，保留未改动消息的协议类型与扩展字段。
fn encode_changed<D: Clone, M: PartialEq>(
    original: &[D],
    edited: &[M],
    decode: impl FnOnce(&[D]) -> Result<Vec<M>>,
    encode: impl FnOnce(&[M]) -> Result<Vec<D>>,
) -> Result<Option<Vec<D>>> {
    let before = decode(original)?;
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
