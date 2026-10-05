//! 测试夹具的 JSON 边界；生产编解码接口只使用协议类型。
use super::{Conversion, EncodeMode, ProtocolCodec, RequestTarget, ResponseTarget};
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
        let typed = crate::protocol::wire::decode_request(*self, &serde_json::to_vec(body)?)?;
        ProtocolCodec::decode_request(self, &typed)
    }
    fn encode_request(&self, ir: &Request) -> Result<Value> {
        serialize_request(
            ProtocolCodec::encode_request(
                self,
                ir,
                &RequestTarget {
                    model: ir.model.as_deref().unwrap_or(""),
                    max_output_tokens: None,
                },
                request_mode(*self, ir),
            )?
            .body,
        )
    }
    fn encode_request_for(
        &self,
        ir: &Request,
        target: &RequestTarget<'_>,
    ) -> Result<Conversion<Value>> {
        let converted = ProtocolCodec::encode_request(self, ir, target, request_mode(*self, ir))?;
        Ok(Conversion {
            body: serialize_request(converted.body)?,
            warnings: converted.warnings,
        })
    }
    fn decode_response(&self, body: &Value) -> Result<Response> {
        let typed = crate::protocol::wire::decode_response(*self, &serde_json::to_vec(body)?)?;
        ProtocolCodec::decode_response(self, &typed)
    }
    fn encode_response(&self, ir: &Response) -> Result<Value> {
        serialize_response(
            ProtocolCodec::encode_response(
                self,
                ir,
                &ResponseTarget {
                    model: ir.model.as_deref().unwrap_or(""),
                    id: ir.id.as_deref().unwrap_or(""),
                    created: ir.created_at.unwrap_or(0),
                },
                response_mode(*self, ir),
            )?
            .body,
        )
    }
    fn encode_response_for(
        &self,
        ir: &Response,
        target: &ResponseTarget<'_>,
    ) -> Result<Conversion<Value>> {
        let converted = ProtocolCodec::encode_response(self, ir, target, response_mode(*self, ir))?;
        Ok(Conversion {
            body: serialize_response(converted.body)?,
            warnings: converted.warnings,
        })
    }
}
/// 历史 JSON 用例显式选择保留模式；跨协议和无来源用例选择重建。
fn request_mode(protocol: Protocol, ir: &Request) -> EncodeMode {
    if protocol == ir.source_protocol() && ir.source.is_some() {
        EncodeMode::Preserve
    } else {
        EncodeMode::Rebuild
    }
}
fn response_mode(protocol: Protocol, ir: &Response) -> EncodeMode {
    if protocol == ir.source_protocol() && ir.source.is_some() {
        EncodeMode::Preserve
    } else {
        EncodeMode::Rebuild
    }
}
fn serialize_request(body: RawRequest) -> Result<Value> {
    Ok(serde_json::to_value(crate::protocol::wire::request(&body))?)
}
fn serialize_response(body: RawResponse) -> Result<Value> {
    Ok(serde_json::to_value(crate::protocol::wire::response(
        &body,
    ))?)
}
