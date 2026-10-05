//! 验证整体 IR 独立于来源类型，编辑通用字段后四协议编码都使用新值。
use super::json_test_support::JsonCodec;

use super::{
    RequestTarget, ResponseTarget,
    cross_tests::{request, response},
};
use crate::{
    ir::{
        request::Function,
        response::{FinishReason, Status},
    },
    protocol::Protocol,
};
use serde_json::{Value, json};

const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

#[test]
fn foreign_protocol_extension_names_are_not_normalized() {
    let mut body = request(Protocol::Gemini);
    body["model"] = json!("extension-model");
    body["stream"] = json!(true);
    let ir = Protocol::Gemini.decode_request(&body).unwrap();
    assert_eq!(ir.model, None);
    assert!(!ir.generation.stream);
    assert_eq!(Protocol::Gemini.encode_request(&ir).unwrap(), body);
    for protocol in [Protocol::Gemini, Protocol::AnthropicMessages] {
        let mut body = response(protocol);
        body["created"] = json!(123);
        let ir = protocol.decode_response(&body).unwrap();
        assert_eq!(ir.created_at, None);
        assert_eq!(protocol.encode_response(&ir).unwrap(), body);
    }
}

#[test]
fn same_protocol_response_envelope_edits_retain_extensions() {
    for protocol in ALL {
        let mut body = response(protocol);
        body["vendor"] = json!({"keep":true});
        let mut ir = protocol.decode_response(&body).unwrap();
        ir.model = Some("edited-model".into());
        ir.id = Some("edited-id".into());
        if matches!(protocol, Protocol::OpenAiChat | Protocol::OpenAiResponses) {
            ir.created_at = Some(456);
        }
        let encoded = protocol.encode_response(&ir).unwrap();
        let decoded = protocol.decode_response(&encoded).unwrap();
        assert_eq!(decoded.model, ir.model);
        assert_eq!(decoded.id, ir.id);
        assert_eq!(decoded.created_at, ir.created_at);
        assert_eq!(encoded["vendor"], body["vendor"]);
    }
}

#[test]
fn detached_requests_encode_all_four_targets_from_edited_ir() {
    for source in ALL {
        let mut ir = source
            .decode_request(&request(source))
            .unwrap()
            .without_source();
        ir.model = Some("ir-model".into());
        ir.generation.max_output_tokens = Some(128);
        ir.generation.temperature = Some(0.7);
        ir.generation.top_p = Some(0.8);
        ir.tools = vec![Function {
            name: "lookup".into(),
            description: Some("Search".into()),
            parameters: json!({"type":"object","properties":{"q":{"type":"string"}}}),
            strict: None,
        }];
        for target in ALL {
            let encoded = target
                .encode_request_for(
                    &ir,
                    &RequestTarget {
                        model: "routed-model",
                        max_output_tokens: None,
                    },
                )
                .unwrap();
            let decoded = target.decode_request(&encoded.body).unwrap();
            assert_eq!(decoded.tools, ir.tools, "{source:?} -> {target:?}");
            assert_eq!(
                decoded.generation, ir.generation,
                "{source:?} -> {target:?}"
            );
            assert_eq!(
                decoded.model.as_deref(),
                if target == Protocol::Gemini {
                    None
                } else {
                    Some("routed-model")
                }
            );
            let direct = target.encode_request(&ir).unwrap();
            if target != Protocol::Gemini {
                assert_eq!(direct["model"], "ir-model");
            }
        }
    }
}

#[test]
fn detached_responses_encode_all_four_targets() {
    for source in ALL {
        let ir = source
            .decode_response(&response(source))
            .unwrap()
            .without_source();
        assert_eq!(ir.status, Status::Completed);
        assert_eq!(ir.candidates.len(), 1);
        for target in ALL {
            let encoded = target
                .encode_response_for(
                    &ir,
                    &ResponseTarget {
                        model: "client-model",
                        id: "ir-response",
                        created: 123,
                    },
                )
                .unwrap();
            let decoded = target.decode_response(&encoded.body).unwrap();
            assert_eq!(decoded.model.as_deref(), Some("client-model"));
            assert_eq!(decoded.id.as_deref(), Some("ir-response"));
            assert_eq!(decoded.status, Status::Completed);
            // 各协议派生的明细不同；当前跨协议保证三个基础用量计数。
            let actual = decoded.usage.as_ref().unwrap();
            let expected = ir.usage.as_ref().unwrap();
            assert_eq!(actual.input_tokens, expected.input_tokens);
            assert_eq!(actual.output_tokens, expected.output_tokens);
            assert_eq!(actual.total_tokens, expected.total_tokens);
        }
    }
}

#[test]
fn manually_built_ir_needs_no_source_type() {
    use crate::ir::{
        message::{Part, PartKind, Role},
        request::{Item, Message, Request},
        response::{Candidate, Response},
    };
    let mut request = Request::new(Protocol::OpenAiChat);
    request.model = Some("model".into());
    request.generation.max_output_tokens = Some(64);
    request.messages.push(Message {
        role: Role::User,
        parts: vec![Part {
            kind: PartKind::Text("hi".into()),
            metadata: Default::default(),
        }],
        metadata: Default::default(),
    });
    request.items.push(Item::Message(0));
    let mut response = Response::new(Protocol::OpenAiChat);
    response.model = Some("model".into());
    response.id = Some("ir-response".into());
    response.created_at = Some(123);
    response.status = Status::Completed;
    let mut message: crate::ir::response::Message = request.messages[0].clone().into();
    message.role = Role::Assistant;
    response.messages.push(message);
    response.items.push(crate::ir::response::Item::Message(0));
    response.candidates.push(Candidate {
        index: 0,
        items: vec![0],
        finish_reason: FinishReason::Stop,
    });
    response.usage = Some(crate::ir::usage::Usage {
        input_tokens: Some(1),
        output_tokens: Some(2),
        total_tokens: Some(3),
        ..Default::default()
    });
    for target in ALL {
        assert!(target.encode_request(&request).is_ok());
        assert!(target.encode_response(&response).is_ok());
    }
}

#[test]
fn same_protocol_edits_common_fields_and_retains_extensions() {
    for protocol in ALL {
        let mut body = request(protocol);
        body["vendor"] = json!({"keep":true});
        let mut ir = protocol.decode_request(&body).unwrap();
        if protocol != Protocol::Gemini {
            ir.model = Some("edited-model".into());
        }
        ir.generation.temperature = Some(0.7);
        ir.generation.max_output_tokens = Some(256);
        ir.tools = vec![Function {
            name: "lookup".into(),
            description: Some("Search".into()),
            parameters: json!({"type":"object"}),
            strict: None,
        }];
        let encoded = protocol.encode_request(&ir).unwrap();
        assert_eq!(encoded["vendor"], body["vendor"]);
        let decoded = protocol.decode_request(&encoded).unwrap();
        assert_eq!(decoded.model, ir.model);
        assert_eq!(decoded.tools, ir.tools);
        assert_eq!(decoded.generation, ir.generation);
    }
}

#[test]
fn source_diagnostics_and_stream_intent_survive_detachment() {
    let mut body = request(Protocol::OpenAiResponses);
    body["previous_response_id"] = json!("private-value");
    let ir = Protocol::OpenAiResponses
        .decode_request(&body)
        .unwrap()
        .without_source();
    let error = Protocol::Gemini
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "m",
                max_output_tokens: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("previous_response_id"));
    assert!(
        !serde_json::to_string(&ir.diagnostics)
            .unwrap()
            .contains("private-value")
    );
    let mut body = request(Protocol::OpenAiChat);
    body["stream"] = json!(true);
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    assert_eq!(Protocol::OpenAiChat.encode_request(&ir).unwrap(), body);
    assert!(ir.generation.stream);
    assert!(
        Protocol::Gemini
            .encode_request(&ir.without_source())
            .is_ok()
    );
}

#[test]
fn candidates_preserve_boundaries_and_do_not_flatten() {
    let mut body = response(Protocol::OpenAiChat);
    let mut second = body["choices"][0].clone();
    second["index"] = json!(1);
    body["choices"].as_array_mut().unwrap().push(second);
    let ir = Protocol::OpenAiChat.decode_response(&body).unwrap();
    assert_eq!(ir.candidates[0].items, vec![0]);
    assert_eq!(ir.candidates[1].items, vec![1]);
    assert_eq!(
        Protocol::Gemini
            .encode_response(&ir.without_source())
            .unwrap()["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn generation_limits_and_loss_are_explicit() {
    let mut ir = Protocol::OpenAiChat
        .decode_request(&request(Protocol::OpenAiChat))
        .unwrap()
        .without_source();
    ir.generation.stop_sequences = Some(vec!["END".into()]);
    let target = RequestTarget {
        model: "m",
        max_output_tokens: None,
    };
    let encoded = Protocol::OpenAiResponses
        .encode_request_for(&ir, &target)
        .unwrap();
    assert!(
        encoded
            .warnings
            .iter()
            .any(|warning| warning.path == "generation.stop_sequences")
    );
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let encoded = protocol.encode_request_for(&ir, &target).unwrap();
        assert_eq!(
            protocol
                .decode_request(&encoded.body)
                .unwrap()
                .generation
                .stop_sequences,
            ir.generation.stop_sequences
        );
    }
    ir.generation.temperature = Some(1.5);
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &target)
            .is_err()
    );
    ir.generation.temperature = Some(0.5);
    ir.generation.candidate_count = Some(2);
    assert_eq!(
        Protocol::Gemini
            .encode_request_for(&ir, &target)
            .unwrap()
            .body["generationConfig"]["candidateCount"],
        2
    );
}

#[test]
fn modifying_readonly_candidate_state_returns_error() {
    let mut ir = Protocol::OpenAiChat
        .decode_response(&response(Protocol::OpenAiChat))
        .unwrap();
    ir.candidates[0].finish_reason = FinishReason::Length;
    assert!(Protocol::OpenAiChat.encode_response(&ir).is_err());
    assert_eq!(
        Protocol::Gemini
            .encode_response(&ir.without_source())
            .unwrap()["candidates"][0]["finishReason"],
        "MAX_TOKENS"
    );
}

#[test]
fn detached_ir_serde_roundtrip_retains_normalized_fields() {
    let ir = Protocol::OpenAiChat
        .decode_request(&request(Protocol::OpenAiChat))
        .unwrap()
        .without_source();
    let value = serde_json::to_value(&ir).unwrap();
    assert_eq!(value["source"], Value::Null);
    let decoded = serde_json::from_value::<crate::ir::request::Request>(value).unwrap();
    assert_eq!(decoded, ir);
}
