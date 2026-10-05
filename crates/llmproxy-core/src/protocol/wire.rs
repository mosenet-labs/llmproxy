//! HTTP 接入边界共用的协议正文读写；不参与 struct 与 IR 的转换。
use super::{Protocol, Request, Response};
use serde::{Serialize, Serializer};

/// 按已知协议解析请求正文，不通过整包 Value 猜测协议。
pub fn decode_request(protocol: Protocol, bytes: &[u8]) -> serde_json::Result<Request> {
    Ok(match protocol {
        Protocol::OpenAiChat => Request::Chat(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::OpenAiResponses => Request::Responses(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::AnthropicMessages => Request::Messages(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::Gemini => Request::Gemini(Box::new(serde_json::from_slice(bytes)?)),
    })
}

/// 按已知协议解析非流式响应正文。
pub fn decode_response(protocol: Protocol, bytes: &[u8]) -> serde_json::Result<Response> {
    Ok(match protocol {
        Protocol::OpenAiChat => Response::Chat(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::OpenAiResponses => Response::Responses(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::AnthropicMessages => Response::Messages(Box::new(serde_json::from_slice(bytes)?)),
        Protocol::Gemini => Response::Gemini(Box::new(serde_json::from_slice(bytes)?)),
    })
}

/// 请求的借用序列化视图；内部枚举标签不会进入 HTTP 正文。
pub fn request(body: &Request) -> impl Serialize + '_ {
    RequestBody(body)
}

/// 响应的借用序列化视图；同一视图可用于 HTTP 客户端或写入字节。
pub fn response(body: &Response) -> impl Serialize + '_ {
    ResponseBody(body)
}

/// 序列化目标请求结构体。
pub fn encode_request(body: &Request) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&request(body))
}

/// 序列化目标非流式响应结构体。
pub fn encode_response(body: &Response) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&response(body))
}

struct RequestBody<'a>(&'a Request);
impl Serialize for RequestBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Request::Chat(body) => body.serialize(serializer),
            Request::Responses(body) => body.serialize(serializer),
            Request::Messages(body) => body.serialize(serializer),
            Request::Gemini(body) => body.serialize(serializer),
        }
    }
}

struct ResponseBody<'a>(&'a Response);
impl Serialize for ResponseBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Response::Chat(body) => body.serialize(serializer),
            Response::Responses(body) => body.serialize(serializer),
            Response::Messages(body) => body.serialize(serializer),
            Response::Gemini(body) => body.serialize(serializer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_protocol_bodies_roundtrip_without_enum_tags() {
        for (protocol, request, response) in [
            (
                Protocol::OpenAiChat,
                r#"{"model":"m","messages":[]}"#,
                r#"{"id":"c","object":"chat.completion","created":1,"model":"m","choices":[]}"#,
            ),
            (
                Protocol::OpenAiResponses,
                r#"{"model":"m","input":"hi"}"#,
                r#"{"id":"r","object":"response","created_at":1,"model":"m","output":[]}"#,
            ),
            (
                Protocol::AnthropicMessages,
                r#"{"model":"m","max_tokens":1,"messages":[]}"#,
                r#"{"id":"m","type":"message","role":"assistant","model":"m","content":[],"usage":{"input_tokens":1,"output_tokens":1}}"#,
            ),
            (
                Protocol::Gemini,
                r#"{"contents":[]}"#,
                r#"{"candidates":[]}"#,
            ),
        ] {
            let body = decode_request(protocol, request.as_bytes()).unwrap();
            let bytes = encode_request(&body).unwrap();
            assert_eq!(decode_request(protocol, &bytes).unwrap(), body);
            assert_eq!(
                serde_json::to_value(super::request(&body)).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
            );
            assert!(
                !serde_json::from_slice::<serde_json::Value>(&bytes)
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .contains_key("Chat")
            );
            let body = decode_response(protocol, response.as_bytes()).unwrap();
            let bytes = encode_response(&body).unwrap();
            assert_eq!(decode_response(protocol, &bytes).unwrap(), body);
            assert_eq!(
                serde_json::to_value(super::response(&body)).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
            );
        }
    }
}
