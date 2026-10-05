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
pub(super) fn convert(
    bytes: Bytes,
    conversion: &CrossConversion,
    tool_state: Option<&crate::tool_state::Context>,
) -> Result<Bytes> {
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
                let mut converted = target.encode_request_for(
                    &request,
                    &RequestTarget {
                        model,
                        max_output_tokens: None,
                    },
                )?;
                if let Some(state) = tool_state {
                    state.restore(&mut converted.body)?;
                }
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
                let mut response = source.decode_response(&body)?;
                if let Some(usage) = &response.usage {
                    crate::observability::response_usage(*source, *target, usage);
                }
                let pending = if let (Some(state), llmproxy_core::protocol::Response::Gemini(raw)) =
                    (tool_state, &body)
                {
                    Some(state.capture(raw, &mut response, *target)?)
                } else {
                    None
                };
                let converted = target.encode_response_for(
                    &response,
                    &ResponseTarget {
                        model,
                        id,
                        created: *created,
                    },
                )?;
                let bytes = codec::encode_response(&converted.body)?;
                if let Some(pending) = pending {
                    pending.commit()?;
                }
                (bytes, converted.warnings)
            }
        };
        for warning in warnings {
            tracing::warn!(component="gateway",event_kind="conversion",source=warning.source.as_str(),target=warning.target.as_str(),path=%warning.path,reason=%warning.reason,"protocol conversion dropped or weakened semantics");
        }
        Ok(Bytes::from(bytes))
    })();
    result.map_err(|_| {
        tracing::warn!(
            component = "gateway",
            event_kind = "conversion",
            phase = if response { "response" } else { "request" },
            "protocol conversion failed"
        );
        Error::explain(
            ErrorType::HTTPStatus(if response { 502 } else { 422 }),
            "protocol conversion failed",
        )
    })
}
