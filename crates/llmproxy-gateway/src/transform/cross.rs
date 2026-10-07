//! Gateway 的整体 JSON 跨协议编解码；只接收路由给定的目标上下文。

use bytes::Bytes;
use llmproxy_core::{
    adapter::protocol_codec::{
        Conversion, ConversionWarning, ProtocolCodec, RequestTarget, ResponseTarget,
    },
    ir::request::Request as IrRequest,
    protocol::{Protocol, Request},
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
    history: Option<&crate::history::Sink>,
) -> Result<Bytes> {
    let response = matches!(conversion, CrossConversion::Response { .. });
    let result = (|| -> std::result::Result<Bytes, llmproxy_core::adapter::Error> {
        let (bytes, warnings) = match conversion {
            CrossConversion::Request {
                source,
                target,
                model,
            } => {
                let mut converted = request(&bytes, *source, *target, model)?;
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
                if let Some(history) = history {
                    history.response(&response);
                }
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
                let converted = target.encode_response(
                    &response,
                    &ResponseTarget {
                        model,
                        id,
                        created: *created,
                    },
                    llmproxy_core::adapter::protocol_codec::EncodeMode::Rebuild,
                )?;
                let bytes = codec::encode_response(&converted.body)?;
                if let Some(pending) = pending {
                    pending.commit()?;
                }
                (bytes, converted.warnings)
            }
        };
        warn(warnings);
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

/// 请求 struct → IR → 目标 struct 后才恢复签名，数据库操作早于正文发送。
pub(super) async fn convert_request(
    bytes: Bytes,
    conversion: &CrossConversion,
    tool_state: Option<&crate::tool_state::Context>,
) -> Result<Bytes> {
    let CrossConversion::Request {
        source,
        target,
        model,
    } = conversion
    else {
        unreachable!()
    };
    let mut converted = request(&bytes, *source, *target, model).map_err(request_error)?;
    finish_request(&mut converted, tool_state).await
}

/// 已解码的请求先构造目标协议，再恢复签名；父请求无需重复解析正文。
pub(super) async fn prepare_request(
    request: &IrRequest,
    conversion: &CrossConversion,
    tool_state: Option<&crate::tool_state::Context>,
) -> Result<Bytes> {
    let CrossConversion::Request {
        source,
        target,
        model,
    } = conversion
    else {
        unreachable!()
    };
    if request.source_protocol() != *source {
        return Err(request_error(llmproxy_core::adapter::Error::Invalid(
            "请求 IR 与来源协议不匹配".into(),
        )));
    }
    let mut converted = encode_request(request, *target, model).map_err(request_error)?;
    finish_request(&mut converted, tool_state).await
}

/// 签名恢复和 HTTP 序列化只在目标结构体上执行，转换警告不含字段值。
async fn finish_request(
    converted: &mut Conversion<Request>,
    tool_state: Option<&crate::tool_state::Context>,
) -> Result<Bytes> {
    if let Some(state) = tool_state {
        state.restore_persisted(&mut converted.body).await?;
    }
    let bytes =
        codec::encode_request(&converted.body).map_err(|error| request_error(error.into()))?;
    if bytes.len() > super::MAX_BUFFERED_BODY {
        return Err(Error::explain(
            ErrorType::HTTPStatus(413),
            "converted request exceeds limit",
        ));
    }
    warn(std::mem::take(&mut converted.warnings));
    Ok(bytes.into())
}

/// JSON 仅在 HTTP 边界解码；目标构造不读取来源 JSON 或来源副本。
fn request(
    bytes: &[u8],
    source: Protocol,
    target: Protocol,
    model: &str,
) -> std::result::Result<Conversion<Request>, llmproxy_core::adapter::Error> {
    let request = decode_request(bytes, source)?;
    encode_request(&request, target, model)
}

/// HTTP 边界读成来源 struct，再投影 IR；不解释客户端 Accept 来猜流式意图。
pub(super) fn decode_request(
    bytes: &[u8],
    source: Protocol,
) -> std::result::Result<IrRequest, llmproxy_core::adapter::Error> {
    let body = codec::decode_request(source, bytes)?;
    source.decode_request(&body)
}

/// 请求的流式标记和消息使用同一套目标协议构造规则。
fn encode_request(
    request: &IrRequest,
    target: Protocol,
    model: &str,
) -> std::result::Result<Conversion<Request>, llmproxy_core::adapter::Error> {
    if target == request.source_protocol() {
        // 同协议只回写思考与路由模型，原生工具、音频及扩展字段保留在来源 struct。
        let mut request = request.clone();
        if target == Protocol::Gemini {
            // Gemini 的模型和流式方法属于 URL，不能要求正文 struct 表达路径字段。
            // 实际流式意图已经保存到 SelectedRoute，此副本只参与正文往返校验。
            request.generation.stream = false;
        } else {
            request.model = Some(model.into());
        }
        return target.encode_request(
            &request,
            &RequestTarget {
                model,
                max_output_tokens: None,
            },
            llmproxy_core::adapter::protocol_codec::EncodeMode::Preserve,
        );
    }
    target.encode_request(
        request,
        &RequestTarget {
            model,
            max_output_tokens: None,
        },
        llmproxy_core::adapter::protocol_codec::EncodeMode::Rebuild,
    )
}

/// 转换错误与存储错误分开处理，拒绝时不输出原始正文。
pub(super) fn request_error(_: llmproxy_core::adapter::Error) -> Box<Error> {
    tracing::warn!(
        component = "gateway",
        event_kind = "conversion",
        phase = "request",
        "protocol conversion failed"
    );
    Error::explain(ErrorType::HTTPStatus(422), "protocol conversion failed")
}

/// 两种执行阶段使用一致的字段级诊断，不记录被丢弃字段的内容。
pub(super) fn warn(warnings: Vec<ConversionWarning>) {
    for warning in warnings {
        tracing::warn!(component="gateway",event_kind="conversion",source=warning.source.as_str(),target=warning.target.as_str(),path=%warning.path,reason=%warning.reason,"protocol conversion dropped or weakened semantics");
    }
}
