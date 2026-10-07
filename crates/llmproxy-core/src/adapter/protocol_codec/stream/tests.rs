use super::Decoder;
use crate::{
    adapter::protocol_codec::ProtocolCodec,
    ir::{
        response::{FinishReason, Status},
        stream::{Event, Limits},
    },
    protocol::{Protocol, stream::Event as Raw},
};
use serde_json::{Value, json};

fn raw(protocol: Protocol, value: Value) -> Raw {
    let bytes = serde_json::to_vec(&value).unwrap();
    match protocol {
        Protocol::OpenAiChat => Raw::Chat(Box::new(serde_json::from_slice(&bytes).unwrap())),
        Protocol::OpenAiResponses => {
            Raw::Responses(Box::new(serde_json::from_slice(&bytes).unwrap()))
        }
        Protocol::AnthropicMessages => {
            Raw::Messages(Box::new(serde_json::from_slice(&bytes).unwrap()))
        }
        Protocol::Gemini => Raw::Gemini(Box::new(serde_json::from_slice(&bytes).unwrap())),
    }
}

fn push(decoder: &mut Decoder, protocol: Protocol, value: Value) -> Vec<Event> {
    decoder.push(&raw(protocol, value)).unwrap()
}

fn chat(choices: Value) -> Value {
    json!({"id":"c1","created":1,"model":"m","object":"chat.completion.chunk","choices":choices})
}

#[test]
fn chat_delivers_text_before_finish_and_usage_after_candidate_end() {
    let protocol = Protocol::OpenAiChat;
    let mut decoder = protocol.stream_decoder(Limits::default());
    let first = push(
        &mut decoder,
        protocol,
        chat(json!([{"index":0,"delta":{"role":"assistant","content":"你"},"finish_reason":null}])),
    );
    assert!(
        first
            .iter()
            .any(|event| matches!(event, Event::TextDelta {text, ..} if text == "你"))
    );
    assert_eq!(decoder.state().ended(), None);
    let next = push(
        &mut decoder,
        protocol,
        chat(json!([{"index":0,"delta":{"content":"好"},"finish_reason":"stop"}])),
    );
    assert!(next.iter().any(|event| matches!(
        event,
        Event::CandidateEnd {
            reason: FinishReason::Stop,
            ..
        }
    )));
    let mut usage = chat(json!([]));
    usage["usage"] = json!({"prompt_tokens":5,"completion_tokens":2,"total_tokens":7,"prompt_tokens_details":{"cached_tokens":3}});
    push(&mut decoder, protocol, usage);
    decoder.push(&Raw::End(protocol)).unwrap();
    assert_eq!(
        decoder
            .state()
            .usage()
            .snapshot()
            .unwrap()
            .cache
            .read_input_tokens,
        Some(3)
    );
    assert_eq!(decoder.state().ended(), Some(Status::Completed));
    assert!(decoder.push(&Raw::End(protocol)).unwrap().is_empty());
}

#[test]
fn chat_reasoning_extension_enters_ir_and_rejects_malformed_or_late_text() {
    use crate::ir::stream::Head;
    let protocol = Protocol::OpenAiChat;
    let mut decoder = protocol.stream_decoder(Limits::default());
    let events = push(
        &mut decoder,
        protocol,
        chat(json!([
            {"index":0,"delta":{"reasoning_content":"分析"}},
            {"index":1,"delta":{"reasoning_content":"另一个候选"}}
        ])),
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                Event::PartStart {
                    head: Head::Reasoning,
                    ..
                }
            ))
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::TextDelta { text, .. } if text == "分析"))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Diagnostic { .. }))
    );
    push(
        &mut decoder,
        protocol,
        chat(json!([{"index":0,"delta":{"content":"回答"},"finish_reason":"stop"}])),
    );
    assert!(
        decoder
            .push(&raw(
                protocol,
                chat(json!([{"index":0,"delta":{"reasoning_content":"晚到的思考"}}]))
            ))
            .is_err()
    );
    let mut decoder = protocol.stream_decoder(Limits::default());
    assert!(
        decoder
            .push(&raw(
                protocol,
                chat(json!([{"index":0,"delta":{"reasoning_content":{"private":"invalid"}}}]))
            ))
            .is_err()
    );
}

#[test]
fn chat_tools_keep_interleaved_names_and_partial_arguments() {
    let protocol = Protocol::OpenAiChat;
    let mut decoder = protocol.stream_decoder(Limits::default());
    push(
        &mut decoder,
        protocol,
        chat(
            json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"get_","arguments":"{\"x\":"}},{"index":1,"id":"b","function":{"name":"other","arguments":"{}"}}]}}]),
        ),
    );
    let tools = decoder.state().active_keys(0).collect::<Vec<_>>();
    assert_eq!(tools.len(), 2);
    assert_eq!(decoder.state().part(tools[0]).unwrap().arguments, "{\"x\":");
    push(
        &mut decoder,
        protocol,
        chat(
            json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"value","arguments":"1}"}}]},"finish_reason":"tool_calls"}]),
        ),
    );
    decoder.push(&Raw::End(protocol)).unwrap();
    assert_eq!(
        decoder.state().candidate(0),
        Some(Some(FinishReason::ToolCall))
    );
    assert_eq!(decoder.state().buffered_bytes(), 0);
}

#[test]
fn messages_merges_raw_cache_counters_before_deriving_total_input() {
    let protocol = Protocol::AnthropicMessages;
    let mut decoder = protocol.stream_decoder(Limits::default());
    push(&mut decoder, protocol, json!({"type":"ping"}));
    push(
        &mut decoder,
        protocol,
        json!({"type":"message_start","message":{"id":"m1","type":"message","role":"assistant","model":"m","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":20,"output_tokens":0,"cache_read_input_tokens":50,"cache_creation_input_tokens":30,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20}}}}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"lookup","input":{}}}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"1}"}}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"type":"content_block_stop","index":0}),
    );
    let update = json!({"type":"message_delta","delta":{},"usage":{"output_tokens":3,"cache_creation":{"ephemeral_5m_input_tokens":10}}});
    push(&mut decoder, protocol, update.clone());
    push(&mut decoder, protocol, update);
    let usage = decoder.state().usage().snapshot().unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.total_tokens, Some(103));
    assert_eq!(usage.cache.write_long_input_tokens, Some(20));
    push(
        &mut decoder,
        protocol,
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
    );
    push(&mut decoder, protocol, json!({"type":"message_stop"}));
    assert_eq!(decoder.state().ended(), Some(Status::Completed));
}

fn response(output: Value) -> Value {
    json!({"id":"r1","model":"m","created_at":1,"object":"response","output":output})
}

#[test]
fn responses_done_snapshots_do_not_repeat_delivered_text() {
    let protocol = Protocol::OpenAiResponses;
    let mut decoder = protocol.stream_decoder(Limits::default());
    let part = json!({"type":"output_text","text":"hello","annotations":[]});
    let message = json!({"id":"o1","type":"message","role":"assistant","status":"completed","content":[part.clone()]});
    let sequence = [
        json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":"o1","type":"message","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.content_part.added","sequence_number":2,"output_index":0,"content_index":0,"item_id":"o1","part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","sequence_number":3,"output_index":0,"content_index":0,"item_id":"o1","delta":"hello"}),
        json!({"type":"response.output_text.done","sequence_number":4,"output_index":0,"content_index":0,"item_id":"o1","text":"hello"}),
        json!({"type":"response.content_part.done","sequence_number":5,"output_index":0,"content_index":0,"item_id":"o1","part":part}),
        json!({"type":"response.output_item.done","sequence_number":6,"output_index":0,"item":message.clone()}),
        json!({"type":"response.completed","sequence_number":7,"response":response(json!([message]))}),
    ];
    let mut text = String::new();
    for raw in sequence {
        for event in push(&mut decoder, protocol, raw) {
            if let Event::TextDelta { text: delta, .. } = event {
                text.push_str(&delta);
            }
        }
    }
    assert_eq!(text, "hello");
    assert_eq!(decoder.state().ended(), Some(Status::Completed));
    assert_eq!(decoder.state().buffered_bytes(), 0);
}

#[test]
fn gemini_merges_adjacent_text_and_keeps_tool_signature_private() {
    let protocol = Protocol::Gemini;
    let mut decoder = protocol.stream_decoder(Limits::default());
    for text in ["a", "b"] {
        push(
            &mut decoder,
            protocol,
            json!({"responseId":"g1","modelVersion":"m","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":text}]}}]}),
        );
    }
    assert_eq!(decoder.state().active_keys(0).count(), 1);
    let events = push(
        &mut decoder,
        protocol,
        json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","id":"a","args":{"q":1}},"thoughtSignature":"private-signature"},{"functionCall":{"name":"other","id":"b","args":{}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2,"cachedContentTokenCount":3,"totalTokenCount":7}}),
    );
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Signature {data, ..} if data == "private-signature")
        )
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::TextDelta {text, ..} if text.contains("private")))
    );
    assert_eq!(
        decoder.state().candidate(0),
        Some(Some(FinishReason::ToolCall))
    );
    decoder.push(&Raw::End(protocol)).unwrap();
    assert_eq!(
        decoder
            .state()
            .usage()
            .snapshot()
            .unwrap()
            .cache
            .read_input_tokens,
        Some(3)
    );
}

#[test]
fn unexpected_eof_protocol_mismatch_and_sequence_rollback_fail_closed() {
    for protocol in [
        Protocol::OpenAiChat,
        Protocol::OpenAiResponses,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let mut decoder = protocol.stream_decoder(Limits::default());
        assert!(decoder.push(&Raw::End(protocol)).is_err());
        assert!(decoder.push(&Raw::End(protocol)).is_err());
        let mut decoder = protocol.stream_decoder(Limits::default());
        let initial = match protocol {
            Protocol::OpenAiChat => chat(json!([{"index":0,"delta":{"content":"partial"}}])),
            Protocol::OpenAiResponses => {
                json!({"type":"response.created","sequence_number":0,"response":response(json!([]))})
            }
            Protocol::AnthropicMessages => {
                json!({"type":"message_start","message":{"type":"message","id":"m","model":"m","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}})
            }
            Protocol::Gemini => json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}),
        };
        push(&mut decoder, protocol, initial);
        assert!(decoder.push(&Raw::End(protocol)).is_err());
        assert_eq!(decoder.state().ended(), None);
    }
    let mut decoder = Protocol::OpenAiChat.stream_decoder(Limits::default());
    assert!(decoder.push(&raw(Protocol::Gemini, json!({}))).is_err());
    assert!(
        decoder
            .push(&raw(Protocol::OpenAiChat, chat(json!([]))))
            .is_err()
    );
    let mut decoder = Protocol::OpenAiResponses.stream_decoder(Limits::default());
    let start =
        json!({"type":"response.created","sequence_number":1,"response":response(json!([]))});
    push(&mut decoder, Protocol::OpenAiResponses, start.clone());
    assert!(
        decoder
            .push(&raw(Protocol::OpenAiResponses, start))
            .is_err()
    );
}

#[test]
fn gemini_partial_usage_uses_prior_prompt_and_invalidates_stale_total() {
    let protocol = Protocol::Gemini;
    let mut decoder = protocol.stream_decoder(Limits::default());
    push(
        &mut decoder,
        protocol,
        json!({"candidates":[{"content":{"parts":[{"text":"x"}]}}],"usageMetadata":{"promptTokenCount":5,"cachedContentTokenCount":3,"candidatesTokenCount":2,"thoughtsTokenCount":1,"totalTokenCount":8}}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"usageMetadata":{"totalTokenCount":10}}),
    );
    assert_eq!(
        decoder.state().usage().snapshot().unwrap().output_tokens,
        Some(5)
    );
    push(
        &mut decoder,
        protocol,
        json!({"usageMetadata":{"candidatesTokenCount":4,"thoughtsTokenCount":2}}),
    );
    let usage = decoder.state().usage().snapshot().unwrap();
    assert_eq!(usage.output_tokens, Some(6));
    assert_eq!(usage.total_tokens, None);
    assert_eq!(usage.cache.read_input_tokens, Some(3));
    assert_eq!(usage.input_details.uncached_tokens, Some(2));
}

#[test]
fn responses_tool_done_does_not_duplicate_arguments_or_confuse_call_id() {
    let protocol = Protocol::OpenAiResponses;
    let mut decoder = protocol.stream_decoder(Limits::default());
    let call = |arguments: &str| json!({"type":"function_call","id":"item-1","call_id":"call-1","name":"lookup","arguments":arguments});
    let sequence = [
        json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":call("")}),
        json!({"type":"response.function_call_arguments.delta","sequence_number":2,"output_index":0,"item_id":"item-1","delta":"{\"x\":"}),
        json!({"type":"response.function_call_arguments.delta","sequence_number":3,"output_index":0,"item_id":"item-1","delta":"1}"}),
        json!({"type":"response.function_call_arguments.done","sequence_number":4,"output_index":0,"item_id":"item-1","arguments":"{\"x\":1}"}),
        json!({"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":call("{\"x\":1}")}),
        json!({"type":"response.completed","sequence_number":6,"response":response(json!([call("{\"x\":1}")]))}),
    ];
    let mut arguments = String::new();
    for source in sequence {
        for event in push(&mut decoder, protocol, source) {
            match event {
                Event::PartStart {
                    head: crate::ir::stream::Head::Tool(head),
                    ..
                } => assert_eq!(head.id.as_deref(), Some("call-1")),
                Event::ToolDelta {
                    arguments: Some(delta),
                    ..
                } => arguments.push_str(&delta),
                _ => {}
            }
        }
    }
    assert_eq!(arguments, "{\"x\":1}");
    assert_eq!(
        decoder.state().candidate(0),
        Some(Some(FinishReason::ToolCall))
    );
}

#[test]
fn citation_snapshot_is_deduplicated_and_unknown_delta_keeps_its_payload() {
    let protocol = Protocol::OpenAiResponses;
    let mut decoder = protocol.stream_decoder(Limits::default());
    push(
        &mut decoder,
        protocol,
        json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"type":"response.content_part.added","sequence_number":1,"output_index":0,"content_index":0,"item_id":"item","part":{"type":"output_text","annotations":[],"text":""}}),
    );
    let citation = json!({"type":"url_citation","url":"https://example.com","title":"Example","start_index":0,"end_index":1});
    let annotation = push(
        &mut decoder,
        protocol,
        json!({"type":"response.output_text.annotation.added","sequence_number":2,"output_index":0,"content_index":0,"item_id":"item","annotation_index":0,"annotation":citation.clone()}),
    );
    assert_eq!(
        annotation
            .iter()
            .filter(|event| matches!(event, Event::Annotation { .. }))
            .count(),
        1
    );
    let done = push(
        &mut decoder,
        protocol,
        json!({"type":"response.content_part.done","sequence_number":3,"output_index":0,"content_index":0,"item_id":"item","part":{"type":"output_text","annotations":[citation],"text":""}}),
    );
    assert!(
        !done
            .iter()
            .any(|event| matches!(event, Event::Annotation { .. }))
    );

    let protocol = Protocol::AnthropicMessages;
    let mut decoder = protocol.stream_decoder(Limits::default());
    push(
        &mut decoder,
        protocol,
        json!({"type":"message_start","message":{"type":"message","id":"m","model":"m","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
    );
    push(
        &mut decoder,
        protocol,
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    );
    let future = json!({"type":"content_block_delta","index":0,"delta":{"type":"future_delta","data":[1,2]}});
    let output = push(&mut decoder, protocol, future.clone());
    assert!(matches!(&output[0], Event::Native(event) if event.as_ref() == &raw(protocol, future)));
    let citation = json!({"type":"char_location","cited_text":"a","document_index":0,"start_char_index":0,"end_char_index":1});
    let output = push(
        &mut decoder,
        protocol,
        json!({"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":citation.clone()}}),
    );
    assert!(matches!(&output[0], Event::Annotation {annotation, ..} if *annotation == citation));
}
