use super::json_test_support::JsonCodec;
use serde_json::{Value, json};

use super::{RequestTarget, ResponseTarget};
use crate::{
    ir::{message::PartKind, request::Item as RequestItem, response::Item as ResponseItem},
    protocol::Protocol,
};

const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

pub(super) fn request(protocol: Protocol) -> Value {
    match protocol {
        Protocol::OpenAiChat => {
            json!({"model":"source","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"}],"max_completion_tokens":64})
        }
        Protocol::OpenAiResponses => {
            json!({"model":"source","input":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"}],"max_output_tokens":64})
        }
        Protocol::AnthropicMessages => {
            json!({"model":"source","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"}],"max_tokens":64})
        }
        Protocol::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]},{"role":"model","parts":[{"text":"hello"}]}],"generationConfig":{"maxOutputTokens":64}})
        }
    }
}

pub(super) fn response(protocol: Protocol) -> Value {
    let usage = match protocol {
        Protocol::OpenAiChat => json!({"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}),
        Protocol::OpenAiResponses => json!({"input_tokens":10,"output_tokens":3,"total_tokens":13}),
        Protocol::AnthropicMessages => json!({"input_tokens":10,"output_tokens":3}),
        Protocol::Gemini => {
            json!({"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13})
        }
    };
    match protocol {
        Protocol::OpenAiChat => {
            json!({"id":"source","created":1,"model":"source","object":"chat.completion","choices":[{"finish_reason":"stop","index":0,"message":{"role":"assistant","content":"hello"}}],"usage":usage})
        }
        Protocol::OpenAiResponses => {
            json!({"id":"source","created_at":1,"model":"source","object":"response","status":"completed","output":[{"id":"msg","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hello","annotations":[]}]}],"usage":usage})
        }
        Protocol::AnthropicMessages => {
            json!({"type":"message","id":"source","model":"source","role":"assistant","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":usage})
        }
        Protocol::Gemini => {
            json!({"responseId":"source","modelVersion":"source","candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":usage})
        }
    }
}

#[test]
fn four_by_four_request_matrix() {
    for source in ALL {
        let ir = source.decode_request(&request(source)).unwrap();
        for target in ALL {
            let body = target
                .encode_request_for(
                    &ir,
                    &RequestTarget {
                        model: "target",
                        max_output_tokens: None,
                    },
                )
                .unwrap();
            let decoded = target.decode_request(&body.body).unwrap();
            assert_eq!(decoded.messages.len(), 2, "{source:?} -> {target:?}");
            assert!(
                matches!(&decoded.messages[0].parts[0].kind, PartKind::Text(text) if text == "hi")
            );
            assert!(
                matches!(&decoded.messages[1].parts[0].kind, PartKind::Text(text) if text == "hello")
            );
            if source != target {
                assert_eq!(
                    body.body.get("model").and_then(Value::as_str),
                    if target == Protocol::Gemini {
                        None
                    } else {
                        Some("target")
                    }
                );
                let limit = match target {
                    Protocol::OpenAiChat => body.body.pointer("/max_completion_tokens"),
                    Protocol::OpenAiResponses => body.body.pointer("/max_output_tokens"),
                    Protocol::AnthropicMessages => body.body.pointer("/max_tokens"),
                    Protocol::Gemini => body.body.pointer("/generationConfig/maxOutputTokens"),
                };
                assert_eq!(limit, Some(&json!(64)));
            }
        }
    }
}

#[test]
fn four_by_four_response_matrix() {
    for source in ALL {
        let ir = source.decode_response(&response(source)).unwrap();
        for target in ALL {
            let body = target
                .encode_response_for(
                    &ir,
                    &ResponseTarget {
                        model: "target",
                        id: "new",
                        created: 2,
                    },
                )
                .unwrap();
            let decoded = target.decode_response(&body.body).unwrap();
            assert_eq!(decoded.messages.len(), 1, "{source:?} -> {target:?}");
            assert!(
                matches!(&decoded.messages[0].parts[0].kind, PartKind::Text(text) if text == "hello")
            );
            let usage = decoded.usage.unwrap();
            assert_eq!(usage.input_tokens, Some(10));
            assert_eq!(usage.output_tokens, Some(3));
            assert_eq!(usage.total_tokens, Some(13));
            if source != target {
                assert_eq!(
                    body.body
                        .get("model")
                        .or_else(|| body.body.get("modelVersion"))
                        .and_then(Value::as_str),
                    Some("target")
                );
                assert_eq!(
                    body.body
                        .get("id")
                        .or_else(|| body.body.get("responseId"))
                        .and_then(Value::as_str),
                    Some("new")
                );
            }
        }
    }
}

#[test]
fn rejects_unmapped_request_semantics() {
    let target = RequestTarget {
        model: "target",
        max_output_tokens: None,
    };
    let mut body = request(Protocol::OpenAiResponses);
    body["previous_response_id"] = json!("prev");
    let ir = Protocol::OpenAiResponses.decode_request(&body).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_request_for(&ir, &target)
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.path.contains("previous_response_id"))
    );

    let mut body = request(Protocol::OpenAiResponses);
    body["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"function_call_output","call_id":"c","output":"ok"}));
    let ir = Protocol::OpenAiResponses.decode_request(&body).unwrap();
    assert!(Protocol::Gemini.encode_request_for(&ir, &target).is_err());

    let mut body = request(Protocol::OpenAiChat);
    body["messages"][0]["content"] =
        json!([{"type":"image_url","image_url":{"url":"https://example.com/a.png"}}]);
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    assert!(
        Protocol::Gemini
            .encode_request_for(&ir, &target)
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.path.contains("parts"))
    );

    let mut body = request(Protocol::OpenAiChat);
    body["prompt_cache_key"] = json!("cache-key");
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    assert!(
        Protocol::Gemini
            .encode_request_for(&ir, &target)
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.path.contains("cache"))
    );

    let mut body = request(Protocol::Gemini);
    body["vendor_option"] = json!(true);
    let ir = Protocol::Gemini.decode_request(&body).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_request_for(&ir, &target)
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.path.contains("vendor_option"))
    );

    let mut body = request(Protocol::OpenAiChat);
    body["stream"] = json!(true);
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    assert!(
        Protocol::Gemini
            .encode_request_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("stream")
    );

    let mut body = request(Protocol::AnthropicMessages);
    body.as_object_mut().unwrap().remove("max_tokens");
    let ir = Protocol::AnthropicMessages.decode_request(&body);
    assert!(ir.is_err());
    let mut body = request(Protocol::OpenAiChat);
    body.as_object_mut()
        .unwrap()
        .remove("max_completion_tokens");
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("max_tokens")
    );
}

#[test]
fn rejects_unmapped_response_semantics() {
    let target = ResponseTarget {
        model: "target",
        id: "new",
        created: 2,
    };
    let mut body = response(Protocol::OpenAiChat);
    let second = body["choices"][0].clone();
    body["choices"].as_array_mut().unwrap().push(second);
    let ir = Protocol::OpenAiChat.decode_response(&body).unwrap();
    assert!(
        Protocol::Gemini
            .encode_response_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("choices")
    );

    let mut body = response(Protocol::OpenAiResponses);
    body["status"] = json!("incomplete");
    let ir = Protocol::OpenAiResponses.decode_response(&body).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_response_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("status")
    );

    let mut body = response(Protocol::Gemini);
    body["usageMetadata"]["cachedContentTokenCount"] = json!(2);
    let ir = Protocol::Gemini.decode_response(&body).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_response_for(&ir, &target)
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.path.contains("usage"))
    );

    let mut body = response(Protocol::OpenAiResponses);
    body["output"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"function_call","id":"fc","name":"lookup","arguments":"{}"}));
    let ir = Protocol::OpenAiResponses.decode_response(&body).unwrap();
    assert!(
        Protocol::Gemini
            .encode_response_for(&ir, &target)
            .unwrap()
            .body
            .pointer("/candidates/0/content/parts/1/functionCall/name")
            .is_some()
    );

    let mut body = response(Protocol::OpenAiChat);
    body.as_object_mut().unwrap().remove("usage");
    let ir = Protocol::OpenAiChat.decode_response(&body).unwrap();
    assert!(
        Protocol::AnthropicMessages
            .encode_response_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("usage")
    );
}

#[test]
fn maps_top_level_and_mid_conversation_roles() {
    let source = json!({"model":"source","instructions":"top", "input":[
        {"role":"system","content":"s1"}, {"role":"developer","content":"d1"},
        {"role":"user","content":"hi"}, {"role":"system","content":"s2"},
        {"role":"developer","content":"d2"}, {"role":"assistant","content":"hello"}
    ],"max_output_tokens":64});
    let ir = Protocol::OpenAiResponses.decode_request(&source).unwrap();
    assert_eq!(ir.instructions[0].text, "top");
    for target in [Protocol::AnthropicMessages, Protocol::Gemini] {
        let converted = target
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        let decoded = target.decode_request(&converted.body).unwrap();
        assert!(decoded.instructions[0].text.contains("top"));
        assert_eq!(decoded.messages[1].role, crate::ir::message::Role::User);
        assert_eq!(decoded.messages[2].role, crate::ir::message::Role::User);
        assert!(
            converted
                .warnings
                .iter()
                .any(|warning| warning.reason.contains("优先级"))
        );
    }
    let chat = Protocol::OpenAiChat
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    assert_eq!(chat.body["messages"][0]["content"], "top");
    assert_eq!(chat.body["messages"][2]["role"], "developer");
    assert!(
        chat.warnings
            .iter()
            .any(|warning| warning.path == "instructions")
    );
}

#[test]
fn maps_client_function_round_trip_across_four_protocols() {
    let source = json!({"model":"source","max_completion_tokens":64,"messages":[
        {"role":"user","content":"search"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"x\"}"}}]},
        {"role":"tool","tool_call_id":"c1","content":"ok"},
        {"role":"user","content":"continue"}
    ],"tools":[{"type":"function","function":{"name":"lookup","description":"Lookup","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}}]});
    let ir = Protocol::OpenAiChat.decode_request(&source).unwrap();
    for target in ALL
        .into_iter()
        .filter(|target| *target != Protocol::OpenAiChat)
    {
        let converted = target
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        let decoded = target.decode_request(&converted.body).unwrap();
        assert!(!decoded.messages.is_empty(), "{target:?}");
        assert!(converted.body.get("tools").is_some(), "{target:?}");
        if target == Protocol::OpenAiResponses {
            assert!(decoded.items.iter().any(|item| matches!(item, RequestItem::ToolCall { call, .. } if call.id.as_deref() == Some("c1"))));
            assert!(decoded.items.iter().any(|item| matches!(item, RequestItem::ToolResult(result) if result.id.as_deref() == Some("c1"))));
        } else {
            assert!(decoded.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, PartKind::ToolCall(call) if call.id.as_deref() == Some("c1"))));
            assert!(decoded.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, PartKind::ToolResult(result) if result.id.as_deref() == Some("c1"))));
        }
    }
}

#[test]
fn maps_responses_independent_function_items() {
    let source = json!({"model":"source","max_output_tokens":64,"input":[
        {"role":"user","content":"search"},
        {"type":"function_call","id":"fc1","call_id":"c1","name":"lookup","arguments":"{}"},
        {"type":"function_call_output","call_id":"c1","output":"ok"}
    ]});
    let ir = Protocol::OpenAiResponses.decode_request(&source).unwrap();
    assert_eq!(ir.items.len(), 3);
    for target in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let converted = target
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        let decoded = target.decode_request(&converted.body).unwrap();
        assert!(decoded.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, PartKind::ToolResult(result) if result.id.as_deref() == Some("c1"))));
    }
}

#[test]
fn maps_tool_call_responses() {
    let source = json!({"id":"source","created":1,"model":"source","object":"chat.completion","choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}});
    let ir = Protocol::OpenAiChat.decode_response(&source).unwrap();
    for target in [
        Protocol::OpenAiResponses,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let converted = target
            .encode_response_for(
                &ir,
                &ResponseTarget {
                    model: "target",
                    id: "new",
                    created: 2,
                },
            )
            .unwrap();
        let decoded = target.decode_response(&converted.body).unwrap();
        if target == Protocol::OpenAiResponses {
            assert!(decoded.items.iter().any(|item| matches!(item, ResponseItem::ToolCall { call, .. } if call.id.as_deref() == Some("c1"))));
        } else {
            assert!(decoded.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, PartKind::ToolCall(call) if call.id.as_deref() == Some("c1"))));
        }
    }
}

#[test]
fn maps_responses_independent_output_call() {
    let source = json!({"id":"source","created_at":1,"model":"source","object":"response","status":"completed","output":[
        {"type":"function_call","id":"fc1","call_id":"c1","name":"lookup","arguments":"{}","status":"completed"}
    ],"usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13}});
    let ir = Protocol::OpenAiResponses.decode_response(&source).unwrap();
    assert!(
        matches!(&ir.items[0], ResponseItem::ToolCall { call, item_id } if call.id.as_deref() == Some("c1") && item_id.as_deref() == Some("fc1"))
    );
    for target in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let converted = target
            .encode_response_for(
                &ir,
                &ResponseTarget {
                    model: "target",
                    id: "new",
                    created: 2,
                },
            )
            .unwrap();
        let decoded = target.decode_response(&converted.body).unwrap();
        assert!(decoded.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, PartKind::ToolCall(call) if call.id.as_deref() == Some("c1"))));
    }
}

#[test]
fn maps_function_declarations_from_all_sources() {
    let definitions = [
        (
            Protocol::OpenAiChat,
            json!({"model":"source","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":64,"tools":[{"type":"function","function":{"name":"lookup","description":"Lookup","parameters":{"type":"object"}}}]}),
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model":"source","input":[{"role":"user","content":"hi"}],"max_output_tokens":64,"tools":[{"type":"function","name":"lookup","description":"Lookup","parameters":{"type":"object"}}]}),
        ),
        (
            Protocol::AnthropicMessages,
            json!({"model":"source","messages":[{"role":"user","content":"hi"}],"max_tokens":64,"tools":[{"name":"lookup","description":"Lookup","input_schema":{"type":"object"}}]}),
        ),
        (
            Protocol::Gemini,
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"generationConfig":{"maxOutputTokens":64},"tools":[{"functionDeclarations":[{"name":"lookup","description":"Lookup","parameters":{"type":"OBJECT"}}]}]}),
        ),
    ];
    for (source, body) in definitions {
        let ir = source.decode_request(&body).unwrap().without_source();
        for target in ALL.into_iter().filter(|target| *target != source) {
            let converted = target
                .encode_request_for(
                    &ir,
                    &RequestTarget {
                        model: "target",
                        max_output_tokens: None,
                    },
                )
                .unwrap();
            let name = if target == Protocol::Gemini {
                converted
                    .body
                    .pointer("/tools/0/functionDeclarations/0/name")
            } else if target == Protocol::OpenAiChat {
                converted.body.pointer("/tools/0/function/name")
            } else {
                converted.body.pointer("/tools/0/name")
            };
            assert_eq!(name, Some(&json!("lookup")), "{source:?} -> {target:?}");
            if target == Protocol::Gemini {
                assert_eq!(
                    converted
                        .body
                        .pointer("/tools/0/functionDeclarations/0/parametersJsonSchema/type"),
                    Some(&json!("object"))
                );
            }
            if source == Protocol::Gemini {
                let schema = if target == Protocol::OpenAiChat {
                    converted.body.pointer("/tools/0/function/parameters/type")
                } else if target == Protocol::OpenAiResponses {
                    converted.body.pointer("/tools/0/parameters/type")
                } else {
                    converted.body.pointer("/tools/0/input_schema/type")
                };
                assert_eq!(schema, Some(&json!("object")));
            }
        }
    }
}

#[test]
fn unassigned_gemini_role_is_dropped_with_warning() {
    let body = json!({"contents":[{"parts":[{"text":"ambiguous"}]},{"role":"user","parts":[{"text":"hi"}]}],"generationConfig":{"maxOutputTokens":64}});
    let ir = Protocol::Gemini.decode_request(&body).unwrap();
    let same = Protocol::Gemini
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    assert!(same.warnings.is_empty());
    let other = Protocol::OpenAiChat
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    assert_eq!(other.body["messages"].as_array().unwrap().len(), 1);
    assert!(
        other
            .warnings
            .iter()
            .any(|warning| warning.path.ends_with(".role"))
    );
}

#[test]
fn same_protocol_rejects_unapplied_item_edits() {
    let mut ir = Protocol::OpenAiResponses.decode_request(&json!({"model":"source","input":[{"role":"user","content":"hi"}],"instructions":"top"})).unwrap();
    ir.instructions[0].text = "edited".into();
    assert!(Protocol::OpenAiResponses.encode_request(&ir).is_err());
}

#[test]
fn messages_and_gemini_tool_history_maps_to_chat() {
    let sources = [
        (
            Protocol::AnthropicMessages,
            json!({"model":"source","max_tokens":64,"messages":[
                {"role":"user","content":"search"},
                {"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"lookup","input":{"q":"x"}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"ok"}]}
            ]}),
        ),
        (
            Protocol::Gemini,
            json!({"contents":[
            {"role":"user","parts":[{"text":"search"}]},
            {"role":"model","parts":[{"functionCall":{"id":"c1","name":"lookup","args":{"q":"x"}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"c1","name":"lookup","response":{"result":"ok"}}}]}
        ],"generationConfig":{"maxOutputTokens":64}}),
        ),
    ];
    for (source, body) in sources {
        let ir = source.decode_request(&body).unwrap();
        let converted = Protocol::OpenAiChat
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        assert_eq!(
            converted.body.pointer("/messages/1/tool_calls/0/id"),
            Some(&json!("c1"))
        );
        assert_eq!(
            converted.body.pointer("/messages/2/tool_call_id"),
            Some(&json!("c1"))
        );
    }
}

#[test]
fn messages_and_gemini_tool_responses_map_to_chat() {
    let sources = [
        (
            Protocol::AnthropicMessages,
            json!({"type":"message","id":"source","model":"source","role":"assistant","content":[{"type":"tool_use","id":"c1","name":"lookup","input":{}}],"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":3}}),
        ),
        (
            Protocol::Gemini,
            json!({"responseId":"source","modelVersion":"source","candidates":[{"content":{"role":"model","parts":[{"functionCall":{"id":"c1","name":"lookup","args":{}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13}}),
        ),
    ];
    for (source, body) in sources {
        let ir = source.decode_response(&body).unwrap();
        let converted = Protocol::OpenAiChat
            .encode_response_for(
                &ir,
                &ResponseTarget {
                    model: "target",
                    id: "new",
                    created: 2,
                },
            )
            .unwrap();
        assert_eq!(
            converted.body.pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        assert_eq!(
            converted.body.pointer("/choices/0/message/tool_calls/0/id"),
            Some(&json!("c1"))
        );
    }
}

#[test]
fn name_only_tool_result_gets_a_matching_id() {
    let body = json!({"contents":[
        {"role":"model","parts":[{"functionCall":{"name":"lookup","args":{}}}]},
        {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"result":"ok"}}}]}
    ],"generationConfig":{"maxOutputTokens":64}});
    let ir = Protocol::Gemini.decode_request(&body).unwrap();
    let converted = Protocol::OpenAiChat
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    let call_id = converted
        .body
        .pointer("/messages/0/tool_calls/0/id")
        .unwrap();
    assert_eq!(
        converted.body.pointer("/messages/1/tool_call_id"),
        Some(call_id)
    );
    assert!(
        converted
            .warnings
            .iter()
            .any(|warning| warning.reason.contains("生成"))
    );
}

#[test]
fn repeated_name_only_calls_pair_in_order() {
    let body = json!({"contents":[
        {"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"q":"a"}}}]},
        {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"result":"a"}}}]},
        {"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"q":"b"}}}]},
        {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"result":"b"}}}]}
    ],"generationConfig":{"maxOutputTokens":64}});
    let ir = Protocol::Gemini.decode_request(&body).unwrap();
    let converted = Protocol::OpenAiChat
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    assert_eq!(
        converted.body.pointer("/messages/0/tool_calls/0/id"),
        converted.body.pointer("/messages/1/tool_call_id")
    );
    assert_eq!(
        converted.body.pointer("/messages/2/tool_calls/0/id"),
        converted.body.pointer("/messages/3/tool_call_id")
    );
    assert_ne!(
        converted.body.pointer("/messages/0/tool_calls/0/id"),
        converted.body.pointer("/messages/2/tool_calls/0/id")
    );
}

#[test]
fn parallel_calls_and_results_remain_grouped() {
    let body = json!({"model":"source","max_completion_tokens":64,"messages":[
        {"role":"user","content":"search"},
        {"role":"assistant","content":"checking","tool_calls":[
            {"id":"c1","type":"function","function":{"name":"first","arguments":"{}"}},
            {"id":"c2","type":"function","function":{"name":"second","arguments":"{}"}}
        ]},
        {"role":"tool","tool_call_id":"c1","content":"one"},
        {"role":"tool","tool_call_id":"c2","content":"two"}
    ]});
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    for target in [Protocol::AnthropicMessages, Protocol::Gemini] {
        let converted = target
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None,
                },
            )
            .unwrap();
        let decoded = target.decode_request(&converted.body).unwrap();
        assert_eq!(decoded.messages.len(), 3);
        assert_eq!(
            decoded.messages[1]
                .parts
                .iter()
                .filter(|part| matches!(&part.kind, PartKind::ToolCall(_)))
                .count(),
            2
        );
        assert_eq!(
            decoded.messages[2]
                .parts
                .iter()
                .filter(|part| matches!(&part.kind, PartKind::ToolResult(_)))
                .count(),
            2
        );
    }
    let message_source = json!({"model":"source","max_tokens":64,"messages":[
        {"role":"user","content":"search"},
        {"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"first","input":{}},{"type":"tool_use","id":"c2","name":"second","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"one"},{"type":"tool_result","tool_use_id":"c2","content":"two"}]}
    ]});
    let ir = Protocol::AnthropicMessages
        .decode_request(&message_source)
        .unwrap();
    let chat = Protocol::OpenAiChat
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "target",
                max_output_tokens: None,
            },
        )
        .unwrap();
    assert_eq!(
        chat.body
            .pointer("/messages/1/tool_calls")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(chat.body["messages"].as_array().unwrap().len(), 4);
}

#[test]
fn unfinished_responses_tool_call_is_rejected() {
    let body = json!({"id":"source","created_at":1,"model":"source","object":"response","status":"completed","output":[{"type":"function_call","id":"fc1","call_id":"c1","name":"lookup","arguments":"{}","status":"in_progress"}],"usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13}});
    let ir = Protocol::OpenAiResponses.decode_response(&body).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_response_for(
                &ir,
                &ResponseTarget {
                    model: "target",
                    id: "new",
                    created: 2
                }
            )
            .is_err()
    );
}

#[test]
fn unmapped_ir_message_is_rejected() {
    let mut ir = Protocol::OpenAiChat
        .decode_request(&request(Protocol::OpenAiChat))
        .unwrap();
    ir.messages.push(ir.messages[0].clone());
    assert!(
        Protocol::Gemini
            .encode_request_for(
                &ir,
                &RequestTarget {
                    model: "target",
                    max_output_tokens: None
                }
            )
            .unwrap_err()
            .to_string()
            .contains("items")
    );
}
