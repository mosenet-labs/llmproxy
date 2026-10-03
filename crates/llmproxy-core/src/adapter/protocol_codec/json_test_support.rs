//! 测试夹具的 JSON 边界；生产编解码接口只使用协议类型。
use super::{Conversion, ProtocolCodec, RequestTarget, ResponseTarget};
use crate::{
    adapter::Result,
    ir::{request::Request, response::Response},
    protocol::{Protocol, Request as RawRequest, Response as RawResponse},
};
use serde_json::Value;
/// 保留现有 JSON 夹具断言，所有调用都通过类型化生产接口。
pub(super) trait JsonCodec {
    fn decode_request(&self, body: &Value) -> Result<Request>;
    fn decode_response(&self, body: &Value) -> Result<Response>;
    fn encode_request(&self, request: &Request) -> Result<Value>;
    fn encode_response(&self, response: &Response) -> Result<Value>;
    fn encode_request_for(
        &self,
        request: &Request,
        target: &RequestTarget<'_>,
    ) -> Result<Conversion<Value>>;
    fn encode_response_for(
        &self,
        response: &Response,
        target: &ResponseTarget<'_>,
    ) -> Result<Conversion<Value>>;
}
impl JsonCodec for Protocol {
    fn decode_request(&self, body: &Value) -> Result<Request> {
        let typed = match self {
            Protocol::OpenAiChat => {
                RawRequest::Chat(Box::new(serde_json::from_value(body.clone())?))
            }
            Protocol::OpenAiResponses => {
                RawRequest::Responses(Box::new(serde_json::from_value(body.clone())?))
            }
            Protocol::AnthropicMessages => {
                RawRequest::Messages(Box::new(serde_json::from_value(body.clone())?))
            }
            Protocol::Gemini => RawRequest::Gemini(Box::new(serde_json::from_value(body.clone())?)),
        };
        ProtocolCodec::decode_request(self, &typed)
    }
    fn encode_request(&self, ir: &Request) -> Result<Value> {
        serialize_request(ProtocolCodec::encode_request(self, ir)?)
    }
    fn encode_request_for(
        &self,
        ir: &Request,
        target: &RequestTarget<'_>,
    ) -> Result<Conversion<Value>> {
        let converted = ProtocolCodec::encode_request_for(self, ir, target)?;
        Ok(Conversion {
            body: serialize_request(converted.body)?,
            warnings: converted.warnings,
        })
    }
    fn decode_response(&self, body: &Value) -> Result<Response> {
        let typed = match self {
            Protocol::OpenAiChat => {
                RawResponse::Chat(Box::new(serde_json::from_value(body.clone())?))
            }
            Protocol::OpenAiResponses => {
                RawResponse::Responses(Box::new(serde_json::from_value(body.clone())?))
            }
            Protocol::AnthropicMessages => {
                RawResponse::Messages(Box::new(serde_json::from_value(body.clone())?))
            }
            Protocol::Gemini => {
                RawResponse::Gemini(Box::new(serde_json::from_value(body.clone())?))
            }
        };
        ProtocolCodec::decode_response(self, &typed)
    }
    fn encode_response(&self, ir: &Response) -> Result<Value> {
        serialize_response(ProtocolCodec::encode_response(self, ir)?)
    }
    fn encode_response_for(
        &self,
        ir: &Response,
        target: &ResponseTarget<'_>,
    ) -> Result<Conversion<Value>> {
        let converted = ProtocolCodec::encode_response_for(self, ir, target)?;
        Ok(Conversion {
            body: serialize_response(converted.body)?,
            warnings: converted.warnings,
        })
    }
}
fn serialize_request(body: RawRequest) -> Result<Value> {
    Ok(match body {
        RawRequest::Chat(body) => serde_json::to_value(body)?,
        RawRequest::Responses(body) => serde_json::to_value(body)?,
        RawRequest::Messages(body) => serde_json::to_value(body)?,
        RawRequest::Gemini(body) => serde_json::to_value(body)?,
    })
}
fn serialize_response(body: RawResponse) -> Result<Value> {
    Ok(match body {
        RawResponse::Chat(body) => serde_json::to_value(body)?,
        RawResponse::Responses(body) => serde_json::to_value(body)?,
        RawResponse::Messages(body) => serde_json::to_value(body)?,
        RawResponse::Gemini(body) => serde_json::to_value(body)?,
    })
}
