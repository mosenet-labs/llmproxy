use serde_json::{Value, json};

use super::{ProtocolCodec, RequestTarget, ResponseTarget};
use crate::{ir::message::PartKind, protocol::Protocol};

const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

fn request(protocol: Protocol) -> Value {
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

fn response(protocol: Protocol) -> Value {
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
            let decoded = target.decode_request(&body).unwrap();
            assert_eq!(decoded.messages.len(), 2, "{source:?} -> {target:?}");
            assert!(
                matches!(&decoded.messages[0].parts[0].kind, PartKind::Text(text) if text == "hi")
            );
            assert!(
                matches!(&decoded.messages[1].parts[0].kind, PartKind::Text(text) if text == "hello")
            );
            if source != target {
                assert_eq!(
                    body.get("model").and_then(Value::as_str),
                    if target == Protocol::Gemini {
                        None
                    } else {
                        Some("target")
                    }
                );
                let limit = match target {
                    Protocol::OpenAiChat => body.pointer("/max_completion_tokens"),
                    Protocol::OpenAiResponses => body.pointer("/max_output_tokens"),
                    Protocol::AnthropicMessages => body.pointer("/max_tokens"),
                    Protocol::Gemini => body.pointer("/generationConfig/maxOutputTokens"),
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
            let decoded = target.decode_response(&body).unwrap();
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
                    body.get("model")
                        .or_else(|| body.get("modelVersion"))
                        .and_then(Value::as_str),
                    Some("target")
                );
                assert_eq!(
                    body.get("id")
                        .or_else(|| body.get("responseId"))
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
            .unwrap_err()
            .to_string()
            .contains("previous_response_id")
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
    assert!(Protocol::Gemini.encode_request_for(&ir, &target).is_err());

    let mut body = request(Protocol::OpenAiChat);
    body["prompt_cache_key"] = json!("cache-key");
    let ir = Protocol::OpenAiChat.decode_request(&body).unwrap();
    assert!(
        Protocol::Gemini
            .encode_request_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("cache")
    );

    let mut body = request(Protocol::Gemini);
    body["vendor_option"] = json!(true);
    let ir = Protocol::Gemini.decode_request(&body).unwrap();
    assert!(
        Protocol::OpenAiChat
            .encode_request_for(&ir, &target)
            .unwrap_err()
            .to_string()
            .contains("vendor_option")
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
            .unwrap_err()
            .to_string()
            .contains("usage")
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
            .unwrap_err()
            .to_string()
            .contains("output")
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
