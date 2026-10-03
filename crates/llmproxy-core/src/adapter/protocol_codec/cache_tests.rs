//! 缓存断点的边界、TTL 与目标能力回归。
use super::{RequestTarget, json_test_support::JsonCodec};
use crate::{
    ir::cache::{Breakpoint, CacheLocation},
    protocol::Protocol,
};
use serde_json::json;
const TARGET: RequestTarget<'static> = RequestTarget {
    model: "target",
    max_output_tokens: Some(128),
};

#[test]
fn cache_breakpoints_keep_instruction_message_and_tool_positions() {
    let raw = json!({"model":"m","max_tokens":128,"system":[{"type":"text","text":"rules","cache_control":{"type":"ephemeral","ttl":"1h"}},{"type":"text","text":"dynamic"}],"tools":[{"name":"lookup","input_schema":{"type":"object"},"cache_control":{"type":"ephemeral","ttl":"1h"}}],"messages":[{"role":"user","content":[{"type":"text","text":"prefix","cache_control":{"type":"ephemeral","ttl":"5m"}},{"type":"text","text":"question"}]}]});
    let ir = Protocol::AnthropicMessages.decode_request(&raw).unwrap();
    assert_eq!(ir.cache_breakpoints.len(), 3);
    assert_eq!(
        Protocol::AnthropicMessages.encode_request(&ir).unwrap(),
        raw
    );
    let ir = ir.without_source();
    for target in [
        Protocol::OpenAiChat,
        Protocol::OpenAiResponses,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let encoded = target.encode_request_for(&ir, &TARGET).unwrap();
        let decoded = target.decode_request(&encoded.body).unwrap();
        match target {
            Protocol::AnthropicMessages => {
                assert_eq!(decoded.cache_breakpoints.len(), 3);
                assert_eq!(encoded.body["system"][0]["cache_control"]["ttl"], "1h");
                assert!(encoded.body["system"][1].get("cache_control").is_none());
                assert_eq!(
                    encoded.body["messages"][0]["content"][0]["cache_control"]["ttl"],
                    "5m"
                );
                assert!(encoded.body["messages"][1].get("cache_control").is_none());
            }
            Protocol::OpenAiChat => {
                assert_eq!(decoded.cache_breakpoints.len(), 2);
                assert!(decoded.cache_breakpoints.iter().all(|p| p.ttl.is_none()));
                assert!(
                    encoded
                        .warnings
                        .iter()
                        .any(|w| w.path == "cache.breakpoint.tool")
                );
            }
            Protocol::OpenAiResponses => assert_eq!(decoded.cache_breakpoints.len(), 2),
            Protocol::Gemini => {
                assert!(decoded.cache_breakpoints.is_empty());
                assert!(!encoded.warnings.is_empty());
            }
        }
    }
}

#[test]
fn invalid_cache_indices_duplicate_and_ttl_order_are_rejected() {
    let raw = json!({"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"first","prompt_cache_breakpoint":{"mode":"explicit"}},{"type":"text","text":"second","prompt_cache_breakpoint":{"mode":"explicit"}}]}]});
    let ir = Protocol::OpenAiChat
        .decode_request(&raw)
        .unwrap()
        .without_source();
    assert_eq!(ir.cache_breakpoints.len(), 2);
    let output = Protocol::AnthropicMessages
        .encode_request_for(&ir, &TARGET)
        .unwrap();
    assert_eq!(
        Protocol::AnthropicMessages
            .decode_request(&output.body)
            .unwrap()
            .cache_breakpoints
            .len(),
        2
    );
    let mut changed = ir.clone();
    changed.cache_breakpoints[1].ttl = Some("1h".into());
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&changed, &TARGET)
            .is_err()
    );
    changed.cache_breakpoints[0].ttl = Some("1h".into());
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&changed, &TARGET)
            .is_ok()
    );
    changed
        .cache_breakpoints
        .push(changed.cache_breakpoints[0].clone());
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&changed, &TARGET)
            .is_err()
    );
    changed.cache_breakpoints = vec![Breakpoint {
        location: CacheLocation::Message {
            message: 0,
            part: 9,
        },
        ttl: None,
    }];
    assert!(
        Protocol::Gemini
            .encode_request_for(&changed, &TARGET)
            .is_err()
    );
}

#[test]
fn messages_does_not_accept_more_than_four_breakpoints() {
    let parts=(0..5).map(|i|json!({"type":"text","text":i.to_string(),"prompt_cache_breakpoint":{"mode":"explicit"}})).collect::<Vec<_>>();
    let raw = json!({"model":"m","messages":[{"role":"user","content":parts}]});
    let ir = Protocol::OpenAiChat.decode_request(&raw).unwrap();
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &TARGET)
            .is_err()
    );
}

#[test]
fn automatic_cache_can_share_the_last_explicit_breakpoint() {
    let parts = (0..4)
        .map(|i| json!({"type":"text","text":i.to_string(),"cache_control":{"type":"ephemeral"}}))
        .collect::<Vec<_>>();
    let raw = json!({"model":"m","max_tokens":128,"cache_control":{"type":"ephemeral"},"messages":[{"role":"user","content":parts}]});
    let ir = Protocol::AnthropicMessages
        .decode_request(&raw)
        .unwrap()
        .without_source();
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &TARGET)
            .is_ok()
    );
    let mut ir = ir;
    ir.cache.ttl = Some("1h".into());
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &TARGET)
            .is_err()
    );
}
