//! 非流式跨协议的状态、候选和统计回归。
use super::{ResponseTarget, cross_tests::response, json_test_support::JsonCodec};
use crate::{ir::response::FinishReason, protocol::Protocol};
use serde_json::json;
const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];
const TARGET: ResponseTarget<'static> = ResponseTarget {
    model: "target",
    id: "response",
    created: 123,
};

#[test]
fn gemini_rebuilt_tool_history_retains_signature_on_its_original_call() {
    use super::RequestTarget;
    let raw = json!({"contents":[
        {"role":"user","parts":[{"text":"lookup"}]},
        {"role":"model","parts":[
            {"functionCall":{"id":"call_1","name":"lookup","args":{"q":"a"}},"thoughtSignature":"opaque-signature"},
            {"functionCall":{"id":"call_2","name":"lookup","args":{"q":"b"}}}
        ]},
        {"role":"user","parts":[
            {"functionResponse":{"id":"call_1","name":"lookup","response":{"result":"OK"}}},
            {"functionResponse":{"id":"call_2","name":"lookup","response":{"result":"OK"}}}
        ]}
    ]});
    let ir = Protocol::Gemini
        .decode_request(&raw)
        .unwrap()
        .without_source();
    let encoded = Protocol::Gemini
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    let parts = encoded.body["contents"][1]["parts"].as_array().unwrap();
    assert_eq!(parts[0]["thoughtSignature"], "opaque-signature");
    assert!(parts[1].get("thoughtSignature").is_none());
    assert!(
        !encoded
            .warnings
            .iter()
            .any(|warning| warning.path.contains("thoughtSignature"))
    );
}

#[test]
fn failed_cancelled_and_running_responses_are_not_successful_completions() {
    use crate::ir::response::Status;
    for (status, expected) in [
        ("failed", Status::Failed),
        ("cancelled", Status::Cancelled),
        ("queued", Status::InProgress),
        ("in_progress", Status::InProgress),
    ] {
        let mut raw = response(Protocol::OpenAiResponses);
        raw["status"] = json!(status);
        raw["output"] = json!([]);
        if status == "failed" {
            raw["error"] = json!({"code":"vendor_error","message":"private-provider-detail","extra_info":true});
        }
        let ir = Protocol::OpenAiResponses.decode_response(&raw).unwrap();
        assert_eq!(ir.status, expected);
        assert_eq!(Protocol::OpenAiResponses.encode_response(&ir).unwrap(), raw);
        if status == "failed" {
            assert_eq!(ir.failure.as_ref().unwrap().code, "vendor_error");
            let mut edited = ir.clone();
            edited.failure.as_mut().unwrap().message = "edited".into();
            let encoded = Protocol::OpenAiResponses.encode_response(&edited).unwrap();
            assert_eq!(encoded["error"]["message"], "edited");
            assert_eq!(encoded["error"]["extra_info"], true);
        }
        let ir = ir.without_source();
        for target in ALL {
            let encoded = target.encode_response_for(&ir, &TARGET);
            if target == Protocol::OpenAiResponses
                && matches!(expected, Status::Failed | Status::Cancelled)
            {
                let encoded = encoded.unwrap();
                assert_eq!(encoded.body["status"], status);
                assert!(!encoded.body.to_string().contains("private-provider-detail"));
                assert!(!encoded.body.to_string().contains("vendor_error"));
            } else {
                assert!(encoded.is_err(), "{status} -> {target:?}");
            }
        }
    }
}

#[test]
fn background_requests_are_retained_same_protocol_and_rejected_cross_protocol() {
    use super::{RequestTarget, cross_tests::request};
    let mut raw = request(Protocol::OpenAiResponses);
    raw["background"] = json!(true);
    let ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
    assert_eq!(Protocol::OpenAiResponses.encode_request(&ir).unwrap(), raw);
    for target in ALL
        .into_iter()
        .filter(|target| *target != Protocol::OpenAiResponses)
    {
        assert!(
            target
                .encode_request_for(
                    &ir,
                    &RequestTarget {
                        model: "target",
                        max_output_tokens: None
                    }
                )
                .is_err()
        );
    }
    raw["background"] = json!(false);
    let ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None
                }
            )
            .is_ok()
    );
}

#[test]
fn text_documents_keep_body_and_report_document_semantics_loss() {
    use super::RequestTarget;
    let raw = json!({"model":"source","max_tokens":128,"messages":[{"role":"user","content":[
        {"type":"text","text":"summarize"},
        {"type":"document","title":"notes.txt","source":{"type":"text","media_type":"text/plain","data":"document body"}}
    ]}]});
    let ir = Protocol::AnthropicMessages.decode_request(&raw).unwrap();
    assert_eq!(
        Protocol::AnthropicMessages.encode_request(&ir).unwrap(),
        raw
    );
    for target in ALL
        .into_iter()
        .filter(|target| *target != Protocol::AnthropicMessages)
    {
        let encoded = target
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        let actual = target.decode_request(&encoded.body).unwrap();
        assert!(actual.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, crate::ir::message::PartKind::Text(text) if text == "document body")));
        assert!(
            encoded
                .warnings
                .iter()
                .any(|warning| warning.reason.contains("文本文档"))
        );
    }
}

#[test]
fn audio_output_without_format_cannot_be_silently_discarded() {
    let mut raw = response(Protocol::OpenAiChat);
    raw["choices"][0]["message"]["audio"] =
        json!({"id":"audio_1","data":"YQ==","expires_at":100,"transcript":"hello"});
    let ir = Protocol::OpenAiChat.decode_response(&raw).unwrap();
    assert_eq!(Protocol::OpenAiChat.encode_response(&ir).unwrap(), raw);
    for target in ALL
        .into_iter()
        .filter(|target| *target != Protocol::OpenAiChat)
    {
        assert!(target.encode_response_for(&ir, &TARGET).is_err());
    }
}

#[test]
fn gemini_usage_details_are_preserved_or_explicitly_warned() {
    let mut raw = response(Protocol::Gemini);
    raw["usageMetadata"]["cachedContentTokenCount"] = json!(4);
    raw["usageMetadata"]["cacheTokensDetails"] = json!([{"modality":"TEXT","tokenCount":4}]);
    raw["usageMetadata"]["toolUsePromptTokenCount"] = json!(2);
    raw["usageMetadata"]["toolUsePromptTokensDetails"] =
        json!([{"modality":"IMAGE","tokenCount":2}]);
    let ir = Protocol::Gemini
        .decode_response(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let encoded = target.encode_response_for(&ir, &TARGET).unwrap();
        let actual = target
            .decode_response(&encoded.body)
            .unwrap()
            .usage
            .unwrap();
        assert_eq!(actual.input_tokens, Some(10));
        assert_eq!(actual.total_tokens, Some(13));
        assert_eq!(actual.cache.read_input_tokens, Some(4));
        if target == Protocol::Gemini {
            assert_eq!(actual.cache.read_details.text_tokens, Some(4));
            assert_eq!(actual.input_details.tool_details.image_tokens, Some(2));
        } else {
            for path in [
                "usage.cache.read_details",
                "usage.input_details.tool_details",
            ] {
                assert!(encoded.warnings.iter().any(|warning| warning.path == path));
            }
        }
    }
    let mut edited = Protocol::Gemini.decode_response(&raw).unwrap();
    edited
        .usage
        .as_mut()
        .unwrap()
        .cache
        .read_details
        .text_tokens = Some(3);
    edited
        .usage
        .as_mut()
        .unwrap()
        .input_details
        .tool_details
        .image_tokens = None;
    let encoded = Protocol::Gemini.encode_response(&edited).unwrap();
    let actual = Protocol::Gemini
        .decode_response(&encoded)
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(actual.cache.read_details.text_tokens, Some(3));
    assert_eq!(actual.input_details.tool_details.image_tokens, None);
    let mut chat = Protocol::OpenAiChat
        .decode_response(&response(Protocol::OpenAiChat))
        .unwrap();
    chat.usage.as_mut().unwrap().cache.read_details.text_tokens = Some(3);
    assert!(Protocol::OpenAiChat.encode_response(&chat).is_err());
    raw["usageMetadata"]["cacheTokensDetails"] = json!([
        {"modality":"TEXT","tokenCount":u64::MAX}, {"modality":"TEXT","tokenCount":1}
    ]);
    let ir = Protocol::Gemini
        .decode_response(&raw)
        .unwrap()
        .without_source();
    assert!(
        Protocol::OpenAiChat
            .encode_response_for(&ir, &TARGET)
            .is_err()
    );
}

#[test]
fn gemini_missing_candidate_indices_use_array_order() {
    let mut raw = response(Protocol::Gemini);
    let mut second = raw["candidates"][0].clone();
    second["content"]["parts"][0]["text"] = json!("second");
    raw["candidates"].as_array_mut().unwrap().push(second);
    let ir = Protocol::Gemini.decode_response(&raw).unwrap();
    let encoded = Protocol::OpenAiChat
        .encode_response_for(&ir, &TARGET)
        .unwrap();
    assert_eq!(encoded.body["choices"][0]["index"], 0);
    assert_eq!(encoded.body["choices"][1]["index"], 1);
    assert_eq!(encoded.body["choices"][1]["message"]["content"], "second");
}

#[test]
fn incomplete_and_empty_outputs_keep_finish_reason_in_all_targets() {
    for source in ALL {
        let mut raw = response(source);
        match source {
            Protocol::OpenAiChat => {
                raw["choices"][0]["finish_reason"] = json!("length");
                raw["choices"][0]["message"]["content"] = json!("");
            }
            Protocol::OpenAiResponses => {
                raw["status"] = json!("incomplete");
                raw["incomplete_details"] = json!({"reason":"max_output_tokens"});
                raw["output"] = json!([]);
            }
            Protocol::AnthropicMessages => {
                raw["stop_reason"] = json!("max_tokens");
                raw["content"] = json!([]);
            }
            Protocol::Gemini => {
                raw["candidates"][0]["finishReason"] = json!("MAX_TOKENS");
                raw["candidates"][0]["content"]["parts"] = json!([]);
            }
        }
        let ir = source.decode_response(&raw).unwrap().without_source();
        for target in ALL {
            let encoded = target.encode_response_for(&ir, &TARGET).unwrap();
            let decoded = target.decode_response(&encoded.body).unwrap();
            assert_eq!(
                decoded.candidates[0].finish_reason,
                FinishReason::Length,
                "{source:?} -> {target:?}"
            );
        }
    }
}

#[test]
fn cache_and_reasoning_counts_survive_cross_protocol_conversion() {
    let mut raw = response(Protocol::AnthropicMessages);
    raw["usage"] = json!({"input_tokens":20,"cache_read_input_tokens":50,"cache_creation_input_tokens":30,"output_tokens":10,"output_tokens_details":{"thinking_tokens":3}});
    let ir = Protocol::AnthropicMessages
        .decode_response(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let encoded = target.encode_response_for(&ir, &TARGET).unwrap();
        let usage = target
            .decode_response(&encoded.body)
            .unwrap()
            .usage
            .unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(10));
        assert_eq!(usage.cache.read_input_tokens, Some(50));
        assert_eq!(usage.output_details.reasoning_tokens, Some(3));
        if target != Protocol::Gemini {
            assert_eq!(usage.cache.write_input_tokens, Some(30));
        }
    }
}

#[test]
fn candidates_remain_separate_and_single_target_reports_selection() {
    let mut raw = response(Protocol::OpenAiChat);
    let mut second = raw["choices"][0].clone();
    second["index"] = json!(3);
    second["message"]["content"] = json!("second");
    second["finish_reason"] = json!("length");
    raw["choices"].as_array_mut().unwrap().push(second);
    let ir = Protocol::OpenAiChat
        .decode_response(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let encoded = target.encode_response_for(&ir, &TARGET).unwrap();
        let decoded = target.decode_response(&encoded.body).unwrap();
        if matches!(target, Protocol::OpenAiChat | Protocol::Gemini) {
            assert_eq!(decoded.candidates.len(), 2);
            assert_eq!(decoded.candidates[1].index, 3);
            assert_eq!(decoded.candidates[1].finish_reason, FinishReason::Length);
        } else {
            assert_eq!(decoded.candidates.len(), 1);
            assert!(encoded.warnings.iter().any(|w| w.path == "candidates"));
        }
    }
}

#[test]
fn image_and_pdf_sources_cross_all_request_protocols() {
    use super::RequestTarget;
    let target = RequestTarget {
        model: "target",
        max_output_tokens: Some(256),
    };
    for (source, raw) in [
        (
            Protocol::OpenAiChat,
            json!({"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"data:image/png;base64,YQ=="}},{"type":"file","file":{"file_data":"YQ==","filename":"doc.pdf"}}]}]}),
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model":"m","input":[{"role":"user","content":[{"type":"input_text","text":"describe"},{"type":"input_image","image_url":"data:image/png;base64,YQ=="},{"type":"input_file","file_data":"data:application/pdf;base64,YQ==","filename":"doc.pdf"}]}]}),
        ),
        (
            Protocol::AnthropicMessages,
            json!({"model":"m","max_tokens":256,"messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"YQ=="}},{"type":"document","title":"doc.pdf","source":{"type":"base64","media_type":"application/pdf","data":"YQ=="}}]}]}),
        ),
        (
            Protocol::Gemini,
            json!({"contents":[{"role":"user","parts":[{"text":"describe"},{"inlineData":{"mimeType":"image/png","data":"YQ=="}},{"inlineData":{"mimeType":"application/pdf","data":"YQ==","displayName":"doc.pdf"}}]}]}),
        ),
    ] {
        let ir = source.decode_request(&raw).unwrap();
        assert_eq!(
            source.encode_request(&ir).unwrap(),
            raw,
            "same protocol {source:?}"
        );
        for destination in ALL {
            let converted = destination
                .encode_request_for(&ir.clone().without_source(), &target)
                .unwrap();
            let decoded = destination.decode_request(&converted.body).unwrap();
            let media = decoded
                .messages
                .iter()
                .flat_map(|m| &m.parts)
                .filter(|p| matches!(p.kind, crate::ir::message::PartKind::Media(_)))
                .count();
            assert_eq!(media, 2, "{source:?} -> {destination:?}");
        }
    }
}

#[test]
fn provider_file_and_conversation_references_are_not_silently_dropped() {
    use super::RequestTarget;
    let target = RequestTarget {
        model: "target",
        max_output_tokens: Some(256),
    };
    for raw in [
        json!({"model":"m","input":[{"role":"user","content":[{"type":"input_file","file_id":"private-file"}]}]}),
        json!({"model":"m","input":"hi","previous_response_id":"private-response"}),
    ] {
        let ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
        assert!(Protocol::Gemini.encode_request_for(&ir, &target).is_err());
        assert_eq!(Protocol::OpenAiResponses.encode_request(&ir).unwrap(), raw);
    }
}

#[test]
fn tool_and_output_constraints_are_projected_and_encoded() {
    use super::RequestTarget;
    let raw = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":256,
        "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}],
        "tool_choice":{"type":"function","function":{"name":"lookup"}},"parallel_tool_calls":false,
        "response_format":{"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object","properties":{"answer":{"type":"string"}}}}},
        "seed":42,"frequency_penalty":0.3,"presence_penalty":0.4});
    let ir = Protocol::OpenAiChat
        .decode_request(&raw)
        .unwrap()
        .without_source();
    let target = RequestTarget {
        model: "target",
        max_output_tokens: None,
    };
    for protocol in ALL {
        let encoded = protocol.encode_request_for(&ir, &target).unwrap();
        let decoded = protocol.decode_request(&encoded.body).unwrap();
        assert_eq!(decoded.generation.max_output_tokens, Some(256));
        assert!(decoded.generation.output_format.is_some());
        assert!(decoded.generation.tool_choice.is_some());
        if protocol != Protocol::Gemini {
            assert_eq!(decoded.generation.parallel_tool_calls, Some(false));
        }
        if matches!(protocol, Protocol::OpenAiChat | Protocol::Gemini) {
            assert_eq!(decoded.generation.seed, Some(42));
        }
    }
}

#[test]
fn allowed_functions_keep_the_restriction_in_each_protocol() {
    use super::RequestTarget;
    use crate::ir::request::controls::ToolChoice;
    let raw = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":64,
        "tools":[{"type":"function","function":{"name":"lookup"}},{"type":"function","function":{"name":"other"}}],
        "tool_choice":{"type":"allowed_tools","allowed_tools":{"mode":"required","tools":[{"type":"function","function":{"name":"lookup"}}]}}});
    let ir = Protocol::OpenAiChat
        .decode_request(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let encoded = target
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        let decoded = target.decode_request(&encoded.body).unwrap();
        if target == Protocol::AnthropicMessages {
            assert_eq!(decoded.tools.len(), 1);
            assert_eq!(decoded.generation.tool_choice, Some(ToolChoice::Required));
        } else {
            assert_eq!(
                decoded.generation.tool_choice,
                Some(ToolChoice::Allowed {
                    required: true,
                    names: vec!["lookup".into()]
                })
            );
        }
    }
}

#[test]
fn editing_generation_keeps_unrelated_source_settings() {
    use crate::ir::request::controls::OutputFormat;
    let raw = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"max_tokens":64});
    let mut ir = Protocol::OpenAiChat.decode_request(&raw).unwrap();
    ir.generation.max_output_tokens = Some(128);
    let encoded = Protocol::OpenAiChat.encode_request(&ir).unwrap();
    assert_eq!(encoded["max_tokens"], 128);
    assert!(encoded.get("max_completion_tokens").is_none());
    let raw = json!({"model":"m","input":"hi","text":{"format":{"type":"text"},"verbosity":"low","vendor":"keep"}});
    let mut ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
    ir.generation.output_format = Some(OutputFormat::JsonObject);
    let encoded = Protocol::OpenAiResponses.encode_request(&ir).unwrap();
    assert_eq!(encoded["text"]["format"]["type"], "json_object");
    assert_eq!(encoded["text"]["verbosity"], "low");
    assert_eq!(encoded["text"]["vendor"], "keep");
}

#[test]
fn malformed_constraints_do_not_become_unrestricted_requests() {
    use super::RequestTarget;
    let target = RequestTarget {
        model: "target",
        max_output_tokens: Some(128),
    };
    for (protocol, raw) in [
        (
            Protocol::AnthropicMessages,
            json!({"model":"m","max_tokens":128,"messages":[{"role":"user","content":"hi"}],"tool_choice":{"type":"tool"}}),
        ),
        (
            Protocol::Gemini,
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"toolConfig":{"functionCallingConfig":{"mode":"NONE","allowedFunctionNames":["lookup"]}}}),
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model":"m","input":"hi","tool_choice":{"type":"allowed_tools","mode":"unknown","tools":[]}}),
        ),
    ] {
        let ir = protocol.decode_request(&raw).unwrap();
        assert!(
            Protocol::OpenAiChat
                .encode_request_for(&ir, &target)
                .is_err()
        );
        assert_eq!(protocol.encode_request(&ir).unwrap(), raw);
    }
}

#[test]
fn gemini_prompt_filter_is_not_an_unknown_empty_response() {
    let raw = json!({"promptFeedback":{"blockReason":"SAFETY"},"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":0,"totalTokenCount":3}});
    let ir = Protocol::Gemini
        .decode_response(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let output = target.encode_response_for(&ir, &TARGET).unwrap();
        let decoded = target.decode_response(&output.body).unwrap();
        assert!(matches!(
            decoded.candidates[0].finish_reason,
            FinishReason::Filtered | FinishReason::Refusal
        ));
    }
}

#[test]
fn incomplete_function_arguments_only_use_string_argument_targets() {
    let mut raw = response(Protocol::OpenAiResponses);
    raw["status"] = json!("incomplete");
    raw["incomplete_details"] = json!({"reason":"max_output_tokens"});
    raw["output"] = json!([{"type":"function_call","id":"fc","call_id":"call","name":"lookup","arguments":"{", "status":"incomplete"}]);
    let ir = Protocol::OpenAiResponses
        .decode_response(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let result = target.encode_response_for(&ir, &TARGET);
        if matches!(target, Protocol::OpenAiChat | Protocol::OpenAiResponses) {
            let output = result.unwrap();
            assert_eq!(
                target.decode_response(&output.body).unwrap().candidates[0].finish_reason,
                FinishReason::Length
            );
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn audio_and_video_are_not_misrepresented_as_text_or_images() {
    use super::RequestTarget;
    let target = RequestTarget {
        model: "target",
        max_output_tokens: Some(128),
    };
    let raw = json!({"model":"m","messages":[{"role":"user","content":[{"type":"input_audio","input_audio":{"format":"wav","data":"YQ=="}}]}]});
    let ir = Protocol::OpenAiChat
        .decode_request(&raw)
        .unwrap()
        .without_source();
    let encoded = Protocol::Gemini.encode_request_for(&ir, &target).unwrap();
    assert_eq!(
        encoded.body["contents"][0]["parts"][0]["inlineData"]["mimeType"],
        "audio/wav"
    );
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &target)
            .is_err()
    );
    let raw = json!({"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"video/mp4","data":"YQ=="}}]}]});
    let ir = Protocol::Gemini.decode_request(&raw).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_request_for(&ir, &target)
            .is_err()
    );
}

#[test]
fn multimodal_tool_results_keep_media_and_error_flags() {
    use super::RequestTarget;
    let raw = json!({"model":"m","max_tokens":128,"messages":[
        {"role":"assistant","content":[{"type":"tool_use","id":"call","name":"lookup","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"call","is_error":true,"content":[{"type":"text","text":"partial result"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"YQ=="}}]}]}]});
    let ir = Protocol::AnthropicMessages
        .decode_request(&raw)
        .unwrap()
        .without_source();
    let target = RequestTarget {
        model: "target",
        max_output_tokens: None,
    };
    let response = Protocol::OpenAiResponses
        .encode_request_for(&ir, &target)
        .unwrap();
    assert_eq!(
        response.body["input"][1]["output"][2]["type"],
        "input_image"
    );
    let response = Protocol::Gemini.encode_request_for(&ir, &target).unwrap();
    let result = &response.body["contents"][1]["parts"][0]["functionResponse"];
    assert_eq!(result["parts"][0]["inlineData"]["mimeType"], "image/png");
    assert_eq!(result["response"]["error"]["result"], "partial result");
    assert!(
        Protocol::OpenAiChat
            .encode_request_for(&ir, &target)
            .is_err()
    );
    let response = Protocol::AnthropicMessages
        .encode_request_for(&ir, &target)
        .unwrap();
    assert_eq!(response.body["messages"][1]["content"][0]["is_error"], true);
}
