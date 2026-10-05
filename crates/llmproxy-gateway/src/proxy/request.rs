//! 上游请求路径、流式方法和鉴权集中处理，Pingora 回调只传入已选定的上下文。
use super::RequestContext;
use llmproxy_core::protocol::{MessagesAuth, Protocol};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use pingora::{Error, ErrorType, Result};
use pingora_http::RequestHeader;

const MODEL_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// 本次请求的目标协议、模型及流式模式在上游连接前已确定。
pub(super) fn filter(request: &mut RequestHeader, ctx: &RequestContext) -> Result<()> {
    let route = ctx.route.as_ref().expect("request_filter selected route");
    let client_protocol = route.protocol;
    let provider = &route.provider;
    let protocol = provider.protocol;
    let base_path = if protocol == Protocol::Gemini {
        let model = route.upstream_model.as_str();
        let model = model.strip_prefix("models/").unwrap_or(model);
        let encoded = utf8_percent_encode(model, MODEL_SEGMENT);
        let method = if route.stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        format!(
            "{}/{}:{method}",
            provider.upstream_path.trim_end_matches('/'),
            encoded
        )
    } else {
        provider.upstream_path.clone()
    };
    let query = if protocol == client_protocol {
        request
            .uri
            .query()
            .unwrap_or_default()
            .split('&')
            .filter(|part| {
                // Gemini 的模式由方法决定，鉴权只使用选定 Provider 的凭据。
                if protocol != Protocol::Gemini {
                    return true;
                }
                let key = part.split('=').next().unwrap_or_default();
                let key = percent_encoding::percent_decode_str(key).decode_utf8_lossy();
                !matches!(key.as_ref(), "alt" | "key" | "access_token")
            })
            .collect::<Vec<_>>()
            .join("&")
    } else {
        String::new()
    };
    // alt=sse 仅用于 Gemini，不会泄漏到 Chat、Responses 或 Messages 的 URL。
    // 参考：https://ai.google.dev/api/generate-content#method:-models.streamgeneratecontent
    let path = if protocol == Protocol::Gemini && route.stream {
        format!(
            "{base_path}?{query}{}alt=sse",
            if query.is_empty() { "" } else { "&" }
        )
    } else if query.is_empty() {
        base_path
    } else {
        format!("{base_path}?{query}")
    };
    request.set_uri(
        path.parse()
            .map_err(|_| Error::explain(ErrorType::InvalidHTTPHeader, "invalid upstream path"))?,
    );
    request.remove_header("authorization");
    request.remove_header("x-api-key");
    request.remove_header("x-goog-api-key");
    request.insert_header("host", provider.authority())?;
    if protocol != client_protocol {
        request.remove_header("content-length");
        request.remove_header("transfer-encoding");
        request.remove_header("content-encoding");
        request.remove_header("expect");
        if let Some(length) = ctx.request_body.prepared_length() {
            request.insert_header("content-length", length.to_string())?;
        }
        request.insert_header("content-type", "application/json")?;
        request.insert_header(
            "accept",
            if route.stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )?;
        // SSE 分帧及非流式 JSON 转换均读取未压缩字节。
        request.insert_header("accept-encoding", "identity")?;
    } else if let Some(length) = request.headers.get("content-length") {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                Error::explain(ErrorType::InvalidHTTPHeader, "invalid content length")
            })?;
        let adjusted = length
            .checked_add_signed(ctx.request_body.body_delta())
            .ok_or_else(|| {
                Error::explain(
                    ErrorType::InvalidHTTPHeader,
                    "invalid rewritten content length",
                )
            })?;
        request.insert_header("content-length", adjusted.to_string())?;
    }
    request.remove_header("content-md5");
    request.remove_header("digest");
    if protocol != client_protocol {
        request.remove_header("anthropic-version");
        request.remove_header("anthropic-beta");
        request.remove_header("openai-organization");
        request.remove_header("openai-project");
        request.remove_header("x-goog-api-client");
        request.remove_header("x-goog-user-project");
    }
    match protocol {
        Protocol::AnthropicMessages => {
            if provider.messages_auth == MessagesAuth::Bearer {
                request.insert_header("authorization", format!("Bearer {}", provider.secret))?;
            } else {
                request.insert_header("x-api-key", provider.secret.as_str())?;
            }
            if let Some(version) = &provider.anthropic_version {
                request.insert_header("anthropic-version", version.as_str())?;
            }
        }
        Protocol::OpenAiChat | Protocol::OpenAiResponses => {
            request.insert_header("authorization", format!("Bearer {}", provider.secret))?;
        }
        Protocol::Gemini => {
            request.insert_header("x-goog-api-key", provider.secret.as_str())?;
        }
    }
    Ok(())
}
