use super::*;
use crate::{
    adapter::protocol_codec::{EncodeMode, ProtocolCodec, RequestTarget},
    protocol::wire,
};
use serde_json::{Value, json};

fn config() -> Config {
    Config {
        support: Support::Switchable,
        enabled: Reasoning {
            effort: Some("medium".into()),
            ..Default::default()
        },
    }
}

fn input(protocol: Protocol) -> Value {
    match protocol {
        Protocol::OpenAiChat => {
            json!({"model":"alias","messages":[{"role":"user","content":"hi"}],"reasoning_effort":"high","vendor_marker":"keep"})
        }
        Protocol::OpenAiResponses => {
            json!({"model":"alias","input":"hi","reasoning":{"effort":"high"},"vendor_marker":"keep"})
        }
        Protocol::AnthropicMessages => {
            json!({"model":"alias","max_tokens":2048,"messages":[{"role":"user","content":"hi"}],"thinking":{"type":"enabled","budget_tokens":1024},"output_config":{"effort":"high"},"vendor_marker":"keep"})
        }
        Protocol::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"generationConfig":{"thinkingConfig":{"thinkingLevel":"high"}},"vendor_marker":"keep"})
        }
    }
}

#[test]
fn default_preserves_existing_configuration_and_disabled_clears_high_effort() {
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::OpenAiResponses,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let raw =
            wire::decode_request(protocol, &serde_json::to_vec(&input(protocol)).unwrap()).unwrap();
        let mut ir = protocol.decode_request(&raw).unwrap();
        let before = ir.clone();
        Config::default()
            .apply(Choice::Default, protocol, &mut ir)
            .unwrap();
        assert_eq!(ir.generation, before.generation);
        for choice in [Choice::Enabled, Choice::Disabled] {
            config().apply(choice, protocol, &mut ir).unwrap();
            let out = protocol
                .encode_request(
                    &ir,
                    &RequestTarget {
                        model: "alias",
                        max_output_tokens: None,
                    },
                    EncodeMode::Preserve,
                )
                .unwrap();
            let value = serde_json::to_value(wire::request(&out.body)).unwrap();
            assert_eq!(value["vendor_marker"], "keep");
            match (protocol, choice) {
                (Protocol::OpenAiChat, _) => assert_eq!(
                    value["reasoning_effort"],
                    if choice == Choice::Enabled {
                        "medium"
                    } else {
                        "none"
                    }
                ),
                (Protocol::OpenAiResponses, _) => assert_eq!(
                    value["reasoning"]["effort"],
                    if choice == Choice::Enabled {
                        "medium"
                    } else {
                        "none"
                    }
                ),
                (Protocol::AnthropicMessages, _) => {
                    assert_eq!(
                        value["thinking"]["type"],
                        if choice == Choice::Enabled {
                            "adaptive"
                        } else {
                            "disabled"
                        }
                    );
                    if choice == Choice::Disabled {
                        assert!(value["output_config"]["effort"].is_null());
                        assert!(value["thinking"]["budget_tokens"].is_null());
                    }
                }
                (Protocol::Gemini, Choice::Enabled) => assert_eq!(
                    value["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                    "medium"
                ),
                (Protocol::Gemini, _) => {
                    assert_eq!(
                        value["generationConfig"]["thinkingConfig"]["thinkingBudget"],
                        0
                    );
                    assert!(value["generationConfig"]["thinkingConfig"]["thinkingLevel"].is_null());
                }
            }
        }
    }
}

#[test]
fn invalid_presets_and_explicit_unsupported_choices_are_rejected() {
    for support in [Support::Unknown, Support::Unsupported] {
        let c = Config {
            support,
            ..Default::default()
        };
        assert!(c.check(Choice::Default).is_ok());
        assert!(c.check(Choice::Enabled).is_err());
        assert!(c.check(Choice::Disabled).is_err());
    }
    assert!(
        Config {
            support: Support::AlwaysOn,
            ..config()
        }
        .check(Choice::Disabled)
        .is_err()
    );
    let manual = Config {
        support: Support::Switchable,
        enabled: Reasoning {
            mode: Some("enabled".into()),
            budget: Some(1024),
            ..Default::default()
        },
    };
    assert!(manual.validate(&[Protocol::AnthropicMessages]).is_ok());
    assert!(manual.validate(&[Protocol::OpenAiChat]).is_err());
    let mut request = Request::new(Protocol::AnthropicMessages);
    request.generation.max_output_tokens = Some(1024);
    assert!(
        manual
            .apply(Choice::Enabled, Protocol::AnthropicMessages, &mut request)
            .is_err()
    );
    let both = Config {
        enabled: Reasoning {
            budget: Some(2048),
            ..config().enabled
        },
        ..config()
    };
    assert!(both.validate(&[Protocol::Gemini]).is_err());
}

#[test]
fn enabled_requests_target_summaries_and_default_keeps_client_preferences() {
    for protocol in [Protocol::Gemini, Protocol::OpenAiResponses] {
        let mut value = input(protocol);
        if protocol == Protocol::Gemini {
            value["generationConfig"]["thinkingConfig"]["includeThoughts"] = json!(false);
        } else {
            value["reasoning"]["summary"] = json!("detailed");
        }
        let raw = wire::decode_request(protocol, &serde_json::to_vec(&value).unwrap()).unwrap();
        let mut ir = protocol.decode_request(&raw).unwrap();
        let before = ir.generation.clone();
        config().apply(Choice::Default, protocol, &mut ir).unwrap();
        assert_eq!(ir.generation, before);
        config().apply(Choice::Enabled, protocol, &mut ir).unwrap();
        let out = protocol
            .encode_request(
                &ir,
                &RequestTarget {
                    model: "alias",
                    max_output_tokens: None,
                },
                EncodeMode::Preserve,
            )
            .unwrap();
        let value = serde_json::to_value(wire::request(&out.body)).unwrap();
        if protocol == Protocol::Gemini {
            assert_eq!(
                value["generationConfig"]["thinkingConfig"]["includeThoughts"],
                true
            );
        } else {
            assert_eq!(value["reasoning"]["summary"], "detailed");
        }
    }
    let mut ir = Request::new(Protocol::OpenAiChat);
    config()
        .apply(Choice::Enabled, Protocol::OpenAiResponses, &mut ir)
        .unwrap();
    assert_eq!(ir.generation.reasoning.summary.as_deref(), Some("auto"));
    for protocol in [Protocol::OpenAiChat, Protocol::AnthropicMessages] {
        let mut ir = Request::new(protocol);
        config().apply(Choice::Enabled, protocol, &mut ir).unwrap();
        assert_eq!(ir.generation.reasoning.include, None);
        assert_eq!(ir.generation.reasoning.summary, None);
    }
}
