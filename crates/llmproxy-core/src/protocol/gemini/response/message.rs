//! `candidates[].content` 的原始协议结构，不包含 Candidate 元数据。
//! 参考 API：https://ai.google.dev/api/generate-content

// API 的 Content 与 Part 同时用于请求和候选响应，直接复用其字段声明。
pub use crate::protocol::gemini::request::message::{Message, Part, Role};

#[cfg(test)]
mod tests {
    use super::Message;
    use serde_json::json;

    #[test]
    fn response_content_round_trip() {
        let source = json!({"role":"model","parts":[{"text":"hello","thoughtSignature":"sig"},{"functionCall":{"name":"lookup","args":{"q":1}}}]});
        let message: Message = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(message).unwrap(), source);
    }
}
