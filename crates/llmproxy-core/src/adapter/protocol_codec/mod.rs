//! 四种协议的完整 JSON 正文与 IR 之间的同协议转换。

mod request;
mod response;

#[cfg(test)]
mod tests;

use serde_json::Value;

use crate::{
    ir::{request::Request, response::Response},
    protocol::{Protocol, chat, gemini, messages, responses},
};

use super::{Error, Result};

/// 请求及非流式响应的协议编解码契约；尚未规范化的字段由协议类型保留。
pub trait ProtocolCodec {
    /// 将 JSON 请求解析为来源协议类型，再投影为 IR。
    fn decode_request(&self, body: &Value) -> Result<Request>;
    /// 将 IR 写回来源协议类型，并序列化为 JSON 请求。
    fn encode_request(&self, request: &Request) -> Result<Value>;
    /// 将非流式 JSON 响应解析为来源协议类型，再投影为 IR。
    fn decode_response(&self, body: &Value) -> Result<Response>;
    /// 将 IR 写回来源协议类型，并序列化为非流式 JSON 响应。
    fn encode_response(&self, response: &Response) -> Result<Value>;
}

impl ProtocolCodec for Protocol {
    fn decode_request(&self, body: &Value) -> Result<Request> {
        use crate::ir::request::source::Source;
        let source = match self {
            Self::OpenAiChat => Source::Chat(Box::new(serde_json::from_value::<
                chat::request::Request,
            >(body.clone())?)),
            Self::OpenAiResponses => Source::Responses(Box::new(serde_json::from_value::<
                responses::request::Request,
            >(body.clone())?)),
            Self::AnthropicMessages => Source::Messages(Box::new(serde_json::from_value::<
                messages::request::Request,
            >(body.clone())?)),
            Self::Gemini => Source::Gemini(Box::new(serde_json::from_value::<
                gemini::request::Request,
            >(body.clone())?)),
        };
        Ok(Request {
            messages: request::decode_messages(&source)?,
            cache: request::decode_cache(&source),
            source,
        })
    }

    fn encode_request(&self, request: &Request) -> Result<Value> {
        ensure_source(*self, request.source_protocol())?;
        let mut source = request.source.clone();
        request::encode_messages(&mut source, &request.messages)?;
        if request.cache != request::decode_cache(&request.source) {
            request::encode_cache(&mut source, &request.cache);
            if request::decode_cache(&source) != request.cache {
                return Err(Error::Unsupported(
                    "目标协议无法表达修改后的缓存设置".into(),
                ));
            }
        }
        Ok(source.into_body()?)
    }

    fn decode_response(&self, body: &Value) -> Result<Response> {
        use crate::ir::response::source::Source;
        let source = match self {
            Self::OpenAiChat => Source::Chat(Box::new(serde_json::from_value::<
                chat::response::Completion,
            >(body.clone())?)),
            Self::OpenAiResponses => Source::Responses(Box::new(serde_json::from_value::<
                responses::response::Response,
            >(body.clone())?)),
            Self::AnthropicMessages => Source::Messages(Box::new(serde_json::from_value::<
                messages::response::Message,
            >(body.clone())?)),
            Self::Gemini => Source::Gemini(Box::new(serde_json::from_value::<
                gemini::response::Response,
            >(body.clone())?)),
        };
        Ok(Response {
            messages: response::decode_messages(&source)?,
            usage: response::decode_usage(&source),
            source,
        })
    }

    fn encode_response(&self, response: &Response) -> Result<Value> {
        ensure_source(*self, response.source_protocol())?;
        let mut source = response.source.clone();
        response::encode_messages(&mut source, &response.messages)?;
        let original_usage = response::decode_usage(&response.source);
        if response.usage != original_usage {
            response::encode_usage(&mut source, response.usage.as_ref())?;
            if !response::changed_usage_fields_match(
                &original_usage,
                &response.usage,
                &response::decode_usage(&source),
            )? {
                return Err(Error::Unsupported("目标协议无法表达修改后的用量".into()));
            }
        }
        Ok(source.into_body()?)
    }
}

/// 整体 IR 仅允许写回来源协议。
fn ensure_source(target: Protocol, source: Protocol) -> Result<()> {
    if target == source {
        Ok(())
    } else {
        Err(Error::Unsupported("整体报文暂不支持跨协议编码".into()))
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
