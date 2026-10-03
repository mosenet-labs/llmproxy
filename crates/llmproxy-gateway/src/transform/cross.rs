//! Gateway 的整体 JSON 跨协议编解码；只接收路由给定的目标上下文。

use bytes::Bytes;
use llmproxy_core::{
    adapter::protocol_codec::{ProtocolCodec, RequestTarget, ResponseTarget},
    protocol::Protocol,
};
use pingora::{Error, ErrorType, Result};

use super::codec;

pub(super) enum CrossConversion {
    Request {
        source: Protocol,
        target: Protocol,
        model: String,
    },
    Response {
        source: Protocol,
        target: Protocol,
        model: String,
        id: String,
        created: i64,
    },
}

/// 仅记录字段路径和原因；正文及工具结果不进入日志。
pub(super) fn convert(bytes: Bytes, conversion: &CrossConversion) -> Result<Bytes> {
    let response = matches!(conversion, CrossConversion::Response { .. });
    let result = (|| -> std::result::Result<Bytes, llmproxy_core::adapter::Error> {
        let (bytes, warnings) = match conversion {
            CrossConversion::Request {
                source,
                target,
                model,
            } => {
                let body = codec::decode_request(*source, &bytes)?;
                let request = source.decode_request(&body)?;
                let converted = target.encode_request_for(
                    &request,
                    &RequestTarget {
                        model,
                        max_output_tokens: None,
                    },
                )?;
                (codec::encode_request(&converted.body)?, converted.warnings)
            }
            CrossConversion::Response {
                source,
                target,
                model,
                id,
                created,
            } => {
                let body = codec::decode_response(*source, &bytes)?;
                let response = source.decode_response(&body)?;
                let converted = target.encode_response_for(
                    &response,
                    &ResponseTarget {
                        model,
                        id,
                        created: *created,
                    },
                )?;
                (codec::encode_response(&converted.body)?, converted.warnings)
            }
        };
        for warning in warnings {
            tracing::warn!(source=warning.source.as_str(),target=warning.target.as_str(),path=%warning.path,reason=%warning.reason,"protocol conversion dropped or weakened semantics");
        }
        Ok(Bytes::from(bytes))
    })();
    result.map_err(|_| {
        tracing::warn!(
            phase = if response { "response" } else { "request" },
            "protocol conversion failed"
        );
        Error::explain(
            ErrorType::HTTPStatus(if response { 502 } else { 422 }),
            "protocol conversion failed",
        )
    })
}
