//! HTTP 边界的序列化；协议转换层不接收 JSON 或字节。
use llmproxy_core::protocol::{Protocol, Request, Response};
/// 按已知协议解析正文，不通过整包 Value 中转。
pub(super) fn decode_request(protocol: Protocol, bytes: &[u8]) -> serde_json::Result<Request> {
    Ok(match protocol {
        Protocol::OpenAiChat => Request::Chat(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::OpenAiResponses => Request::Responses(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::AnthropicMessages => Request::Messages(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::Gemini => Request::Gemini(Box::new(serde_json::from_slice(bytes)?)),
    })
}
/// 序列化目标协议结构体，枚举标识不进入 HTTP 正文。
pub(super) fn encode_request(body: &Request) -> serde_json::Result<Vec<u8>> {
    match body {
        Request::Chat(body) => serde_json::to_vec(body),
        Request::Responses(body) => serde_json::to_vec(body),
        Request::Messages(body) => serde_json::to_vec(body),
        Request::Gemini(body) => serde_json::to_vec(body),
    }
}
/// 按已知协议解析正文，不通过整包 Value 中转。
pub(super) fn decode_response(protocol: Protocol, bytes: &[u8]) -> serde_json::Result<Response> {
    Ok(match protocol {
        Protocol::OpenAiChat => Response::Chat(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::OpenAiResponses => Response::Responses(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::AnthropicMessages => Response::Messages(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::Gemini => Response::Gemini(Box::new(serde_json::from_slice(bytes)?)),
    })
}
/// 序列化目标协议结构体，枚举标识不进入 HTTP 正文。
pub(super) fn encode_response(body: &Response) -> serde_json::Result<Vec<u8>> {
    match body {
        Response::Chat(body) => serde_json::to_vec(body),
        Response::Responses(body) => serde_json::to_vec(body),
        Response::Messages(body) => serde_json::to_vec(body),
        Response::Gemini(body) => serde_json::to_vec(body),
    }
}
