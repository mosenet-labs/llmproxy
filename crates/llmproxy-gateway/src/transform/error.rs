//! HTTP 边界的类型化错误外壳；不将 Provider 的原始错误正文泄露给客户端。
use bytes::Bytes;
use llmproxy_core::protocol::Protocol;
use pingora::{Result, proxy::Session};
use pingora_http::ResponseHeader;
use serde::Serialize;

#[derive(Serialize)]
struct Envelope<T> {
    error: T,
}
#[derive(Serialize)]
struct ChatError {
    message: &'static str,
    r#type: &'static str,
    code: u16,
}
#[derive(Serialize)]
struct MessagesEnvelope {
    r#type: &'static str,
    error: MessagesError,
}
#[derive(Serialize)]
struct MessagesError {
    r#type: &'static str,
    message: &'static str,
}
#[derive(Serialize)]
struct GeminiError {
    code: u16,
    message: &'static str,
    status: &'static str,
}

/// 将 HTTP 状态映射为各客户端可识别的错误分类，不依赖整包 JSON Value。
pub(super) fn body(protocol: Protocol, status: u16) -> Bytes {
    let message = match status {
        400 | 413 | 415 | 422 => "Request cannot be processed",
        401 | 403 => "Provider authentication or permission failed",
        404 => "Requested model or resource is unavailable",
        429 => "Provider rate limit exceeded",
        _ => "Provider request failed",
    };
    let bytes = match protocol {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => serde_json::to_vec(&Envelope {
            error: ChatError {
                message,
                r#type: match status {
                    400 | 413 | 415 | 422 => "invalid_request_error",
                    401 => "authentication_error",
                    403 => "permission_error",
                    404 => "not_found_error",
                    429 => "rate_limit_error",
                    _ => "upstream_error",
                },
                code: status,
            },
        }),
        Protocol::AnthropicMessages => serde_json::to_vec(&MessagesEnvelope {
            r#type: "error",
            error: MessagesError {
                message,
                r#type: match status {
                    400 | 415 | 422 => "invalid_request_error",
                    413 => "request_too_large",
                    401 => "authentication_error",
                    403 => "permission_error",
                    404 => "not_found_error",
                    429 => "rate_limit_error",
                    529 => "overloaded_error",
                    _ => "api_error",
                },
            },
        }),
        Protocol::Gemini => serde_json::to_vec(&Envelope {
            error: GeminiError {
                message,
                code: status,
                status: match status {
                    400 | 415 | 422 => "INVALID_ARGUMENT",
                    401 => "UNAUTHENTICATED",
                    403 => "PERMISSION_DENIED",
                    404 => "NOT_FOUND",
                    413 | 429 => "RESOURCE_EXHAUSTED",
                    503 | 529 => "UNAVAILABLE",
                    504 => "DEADLINE_EXCEEDED",
                    _ => "INTERNAL",
                },
            },
        }),
    };
    Bytes::from(bytes.expect("固定错误字段可以序列化"))
}

/// 仅在尚未发出响应头时调用，保证状态码与 JSON 错误外壳一致。
pub async fn respond(session: &mut Session, protocol: Option<Protocol>, status: u16) -> Result<()> {
    let Some(protocol) = protocol else {
        return session.respond_error(status).await;
    };
    let body = body(protocol, status);
    let mut header = ResponseHeader::build(status, Some(3))?;
    header.insert_header("content-type", "application/json")?;
    header.insert_header("content-length", body.len().to_string())?;
    header.insert_header("cache-control", "no-store")?;
    session
        .write_response_header(Box::new(header), false)
        .await?;
    session.write_response_body(Some(body), true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rate_limits_have_client_protocol_error_categories() {
        for (protocol, key, value) in [
            (Protocol::OpenAiChat, "type", "rate_limit_error"),
            (Protocol::OpenAiResponses, "type", "rate_limit_error"),
            (Protocol::AnthropicMessages, "type", "rate_limit_error"),
            (Protocol::Gemini, "status", "RESOURCE_EXHAUSTED"),
        ] {
            let json: serde_json::Value = serde_json::from_slice(&body(protocol, 429)).unwrap();
            assert_eq!(json["error"][key], value);
        }
    }
}
