use super::json_test_support::JsonCodec;
use serde_json::{Value, json};

use crate::{ir::message::PartKind, protocol::Protocol};

#[test]
fn all_requests_round_trip_and_update_messages_and_cache() {
    let cases: [(Protocol, Value, &str, &str); 4] = [
        (
            Protocol::OpenAiChat,
            json!({"model":"m","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"old","prompt_cache_options":{"mode":"explicit","ttl":"30m","vendor":1},"temperature":0.2}),
            "/prompt_cache_key",
            "/messages/0/content",
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model":"m","input":[{"role":"user","content":"hi"},{"type":"function_call_output","call_id":"c","output":"ok"}],"prompt_cache_key":"old","prompt_cache_options":{"mode":"explicit","ttl":"30m","vendor":1},"temperature":0.2}),
            "/prompt_cache_key",
            "/input/0/content",
        ),
        (
            Protocol::AnthropicMessages,
            json!({"model":"m","max_tokens":100,"messages":[{"role":"user","content":"hi"}],"cache_control":{"type":"ephemeral","ttl":"5m","vendor":1},"temperature":0.2}),
            "/cache_control/ttl",
            "/messages/0/content",
        ),
        (
            Protocol::Gemini,
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"cachedContent":"cachedContents/old","generationConfig":{"temperature":0.2},"vendor":1}),
            "/cachedContent",
            "/contents/0/parts/0/text",
        ),
    ];
    for (protocol, source, cache_path, message_path) in cases {
        let mut request = protocol.decode_request(&source).unwrap();
        assert_eq!(request.messages.len(), 1);
        assert_eq!(protocol.encode_request(&request).unwrap(), source);
        match protocol {
            Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                request.cache.key = Some("next".into());
            }
            Protocol::AnthropicMessages => request.cache.ttl = Some("1h".into()),
            Protocol::Gemini => request.cache.reference = Some("cachedContents/next".into()),
        }
        let PartKind::Text(text) = &mut request.messages[0].parts[0].kind else {
            panic!("预期请求正文为文本");
        };
        *text = "edited".into();
        let encoded = protocol.encode_request(&request).unwrap();
        assert_ne!(encoded.pointer(cache_path), source.pointer(cache_path));
        assert_eq!(encoded.pointer(message_path), Some(&json!("edited")));
        assert_eq!(encoded["temperature"], source["temperature"]);
        if protocol == Protocol::OpenAiResponses {
            assert_eq!(encoded["input"][1], source["input"][1]);
        }
    }
}

#[test]
fn all_responses_round_trip_and_update_messages_and_usage() {
    let cases: [(Protocol, Value, &str, &str); 4] = [
        (
            Protocol::OpenAiChat,
            json!({"id":"c1","created":1,"model":"m","object":"chat.completion","choices":[{"finish_reason":"stop","index":0,"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":2,"cache_write_tokens":0}},"vendor":1}),
            "/usage/prompt_tokens_details/cached_tokens",
            "/choices/0/message/content",
        ),
        (
            Protocol::OpenAiResponses,
            json!({"id":"r1","created_at":1,"model":"m","object":"response","output":[{"id":"msg_1","content":[{"type":"output_text","annotations":[],"text":"hi"}],"role":"assistant","status":"completed","type":"message"},{"type":"function_call","id":"fc_1","name":"lookup","arguments":"{}"}],"usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":2,"cache_write_tokens":0},"output_tokens":3,"total_tokens":13},"vendor":1}),
            "/usage/input_tokens_details/cached_tokens",
            "/output/0/content/0/text",
        ),
        (
            Protocol::AnthropicMessages,
            json!({"type":"message","id":"m1","content":[{"type":"text","text":"hi"}],"model":"m","role":"assistant","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":8,"output_tokens":3,"cache_read_input_tokens":2},"vendor":1}),
            "/usage/cache_read_input_tokens",
            "/content/0/text",
        ),
        (
            Protocol::Gemini,
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]}},{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"cachedContentTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":13},"vendor":1}),
            "/usageMetadata/cachedContentTokenCount",
            "/candidates/0/content/parts/0/text",
        ),
    ];
    for (protocol, source, usage_path, message_path) in cases {
        let mut response = protocol.decode_response(&source).unwrap();
        assert_eq!(response.messages.len(), 1);
        assert_eq!(
            response.usage.as_ref().unwrap().cache.read_input_tokens,
            Some(2)
        );
        assert_eq!(protocol.encode_response(&response).unwrap(), source);
        response.usage.as_mut().unwrap().cache.read_input_tokens = Some(4);
        let PartKind::Text(text) = &mut response.messages[0].parts[0].kind else {
            panic!("预期响应正文为文本");
        };
        *text = "edited".into();
        let encoded = protocol.encode_response(&response).unwrap();
        assert_eq!(encoded.pointer(usage_path), Some(&json!(4)));
        assert_eq!(encoded.pointer(message_path), Some(&json!("edited")));
        assert_eq!(encoded["vendor"], source["vendor"]);
        if protocol == Protocol::OpenAiResponses {
            assert_eq!(encoded["output"][1], source["output"][1]);
        }
    }
}

#[test]
fn normalized_request_can_encode_another_protocol_but_invalid_response_is_rejected() {
    let source = json!({"model":"m","messages":[{"role":"user","content":"hi"}]});
    let request = Protocol::OpenAiChat.decode_request(&source).unwrap();
    let encoded = Protocol::Gemini.encode_request(&request).unwrap();
    assert_eq!(encoded["contents"][0]["parts"][0]["text"], "hi");
    assert!(
        Protocol::OpenAiChat
            .decode_response(&json!({"error":{"message":"bad"}}))
            .is_err()
    );
}
