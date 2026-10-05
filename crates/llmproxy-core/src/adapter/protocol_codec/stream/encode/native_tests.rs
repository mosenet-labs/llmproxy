//! 原厂服务端输出、音频尾帧及字段诊断的独立边界夹具。
use super::tests::{PROTOCOLS, TARGET, chat, raw, response};
use crate::{
    adapter::protocol_codec::ProtocolCodec,
    ir::stream::{Event, Limits},
    protocol::{Protocol, stream::Event as Raw},
};
use serde_json::{Value, json};

/// 经来源解码、目标编码和目标再解码，观察实际交付而非原始快照。
fn convert(
    source: Protocol,
    target: Protocol,
    values: Vec<Value>,
) -> (String, Vec<Raw>, Vec<String>) {
    let mut decoder = source.stream_decoder(Limits::default());
    let mut encoder = target
        .stream_encoder(source, &TARGET, Limits::default())
        .unwrap();
    let mut target_decoder = target.stream_decoder(Limits::default());
    let mut text = String::new();
    let mut output = Vec::new();
    let mut warnings = Vec::new();
    for raw in values
        .into_iter()
        .map(|value| raw(source, value))
        .chain([Raw::End(source)])
    {
        for event in decoder.push(&raw).unwrap() {
            let converted = encoder.push(&event).unwrap();
            for warning in converted.warnings {
                warnings.push(format!("{}:{}", warning.path, warning.reason));
            }
            for raw in converted.body {
                for event in target_decoder.push(&raw).unwrap() {
                    match event {
                        Event::TextDelta { text: delta, .. } => text.push_str(&delta),
                        Event::ToolDelta { .. } => panic!("服务端执行记录被伪造成客户端工具调用"),
                        _ => {}
                    }
                }
                output.push(raw);
            }
        }
    }
    assert!(decoder.state().ended().is_some());
    assert!(encoder.state().ended().is_some());
    assert!(target_decoder.state().ended().is_some());
    (text, output, warnings)
}

#[test]
fn three_sources_send_server_results_once_to_all_four_targets() {
    let messages = vec![
        json!({"type":"message_start","message":{"id":"source","model":"m","role":"assistant","type":"message","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srv","name":"bash_code_execution"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"private\":\"secret-parameters\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"bash_code_execution_tool_result","tool_use_id":"srv","content":{"type":"bash_code_execution_result","stdout":"visible-result","stderr":"","return_code":0,"content":[{"type":"bash_code_execution_output","file_id":"private-file"}]}}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ];
    let code = json!({"type":"code_interpreter_call","id":"ci","code":"visible-code","outputs":[{"type":"logs","logs":"visible-result"}],"container_id":"private-container","status":"completed"});
    let mcp = json!({"type":"mcp_call","id":"mcp","name":"remote","arguments":"secret-parameters","output":"visible-mcp","server_label":"private-server"});
    let shell = json!({"type":"shell_call_output","call_id":"sh","output":[{"stdout":"visible-shell","stderr":"","outcome":{"type":"exit","exit_code":0}}]});
    let mut final_response = response(json!([code.clone(), mcp.clone(), shell.clone()]));
    final_response["usage"] = json!({"input_tokens":1,"output_tokens":2,"total_tokens":3});
    let responses = vec![
        json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"code_interpreter_call","id":"ci","code":"","status":"in_progress"}}),
        json!({"type":"response.code_interpreter_call_code.delta","sequence_number":2,"output_index":0,"item_id":"ci","delta":"visible-code"}),
        json!({"type":"response.output_item.done","sequence_number":3,"output_index":0,"item":code}),
        json!({"type":"response.output_item.done","sequence_number":4,"output_index":1,"item":mcp}),
        json!({"type":"response.output_item.done","sequence_number":5,"output_index":2,"item":shell}),
        json!({"type":"response.completed","sequence_number":6,"response":final_response}),
    ];
    let gemini = vec![
        json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"executableCode":{"language":"PYTHON","code":"visible-code"}},{"codeExecutionResult":{"outcome":"OUTCOME_OK","output":"visible-result"}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"totalTokenCount":3}}),
    ];
    for (source, values) in [
        (Protocol::AnthropicMessages, messages),
        (Protocol::OpenAiResponses, responses),
        (Protocol::Gemini, gemini),
    ] {
        for target in PROTOCOLS {
            let (text, _, warnings) = convert(source, target, values.clone());
            assert_eq!(
                text.matches("visible-result").count(),
                1,
                "{source:?} -> {target:?}: {text}"
            );
            if source != Protocol::AnthropicMessages {
                assert_eq!(text.matches("visible-code").count(), 1);
            }
            if source == Protocol::OpenAiResponses {
                assert_eq!(text.matches("visible-mcp").count(), 1);
                assert_eq!(text.matches("visible-shell").count(), 1);
            }
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.starts_with("server_output:"))
            );
            for secret in [
                "secret-parameters",
                "private-file",
                "private-container",
                "private-server",
            ] {
                assert!(!text.contains(secret));
                assert!(!warnings.iter().any(|warning| warning.contains(secret)));
            }
        }
    }
}

#[test]
fn chat_audio_tail_keeps_audio_and_does_not_repeat_public_text() {
    let values = vec![
        chat(
            json!([{"index":0,"delta":{"content":"visible-text","audio":{"id":"audio","data":"YQ==","transcript":"spoken"}}}]),
        ),
        chat(json!([{"index":0,"delta":{},"finish_reason":"stop"}])),
        chat(json!([{"index":0,"delta":{"audio":{"expires_at":123}}}])),
    ];
    let (text, output, _) = convert(Protocol::OpenAiChat, Protocol::OpenAiChat, values.clone());
    assert_eq!(text, "visible-text");
    let audio = output
        .iter()
        .filter_map(|raw| {
            if let Raw::Chat(chunk) = raw {
                assert_eq!(chunk.id, "target");
                assert_eq!(chunk.model, "target-model");
                chunk
                    .choices
                    .first()
                    .and_then(|choice| choice.delta.audio.as_option())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0].data.as_option().unwrap(), "YQ==");
    assert_eq!(audio[1].expires_at.as_option(), Some(&123));
    for target in PROTOCOLS
        .into_iter()
        .filter(|target| *target != Protocol::OpenAiChat)
    {
        let mut decoder = Protocol::OpenAiChat.stream_decoder(Limits::default());
        let mut encoder = target
            .stream_encoder(Protocol::OpenAiChat, &TARGET, Limits::default())
            .unwrap();
        let events = decoder
            .push(&raw(Protocol::OpenAiChat, values[0].clone()))
            .unwrap();
        assert!(events.iter().any(|event| encoder.push(event).is_err()));
    }
}

#[test]
fn chat_rejects_missing_duplicate_or_invalid_audio_end_updates() {
    let first = chat(
        json!([{"index":0,"delta":{"audio":{"id":"a","data":"YQ=="}},"finish_reason":"stop"}]),
    );
    let expiry = chat(json!([{"index":0,"delta":{"audio":{"expires_at":123}}}]));
    let mut decoder = Protocol::OpenAiChat.stream_decoder(Limits::default());
    decoder
        .push(&raw(Protocol::OpenAiChat, first.clone()))
        .unwrap();
    assert!(decoder.push(&Raw::End(Protocol::OpenAiChat)).is_err());
    for (i, tail) in [
        expiry.clone(),
        chat(json!([{"index":0,"delta":{"content":"late","audio":{"expires_at":123}}}])),
    ]
    .into_iter()
    .enumerate()
    {
        let mut decoder = Protocol::OpenAiChat.stream_decoder(Limits::default());
        decoder
            .push(&raw(Protocol::OpenAiChat, first.clone()))
            .unwrap();
        let result = decoder.push(&raw(Protocol::OpenAiChat, tail));
        if i == 0 {
            result.unwrap();
            assert!(
                decoder
                    .push(&raw(Protocol::OpenAiChat, expiry.clone()))
                    .is_err()
            );
        } else {
            assert!(result.is_err());
        }
    }
    let mut decoder = Protocol::OpenAiChat.stream_decoder(Limits::default());
    assert!(decoder.push(&raw(Protocol::OpenAiChat, expiry)).is_err());
}

#[test]
fn responses_audio_has_independent_done_and_target_owned_ids_and_sequences() {
    let values = vec![
        json!({"type":"response.created","sequence_number":10,"response":response(json!([]))}),
        json!({"type":"response.audio.delta","sequence_number":11,"response_id":"source","delta":"YQ=="}),
        json!({"type":"response.audio.transcript.delta","sequence_number":12,"delta":"spoken"}),
        json!({"type":"response.audio.done","sequence_number":13}),
        json!({"type":"response.audio.transcript.done","sequence_number":14}),
        json!({"type":"response.completed","sequence_number":15,"response":response(json!([]))}),
    ];
    let (_, output, _) = convert(
        Protocol::OpenAiResponses,
        Protocol::OpenAiResponses,
        values.clone(),
    );
    let mut previous = None;
    for raw in output {
        if let Raw::Responses(event) = raw
            && let crate::protocol::responses::response::Event::Known(event) = *event
        {
            assert!(previous.is_none_or(|previous| event.sequence_number() > previous));
            previous = Some(event.sequence_number());
            match event.as_ref() {
                crate::protocol::responses::response::event::KnownEvent::AudioDelta(delta)
                | crate::protocol::responses::response::event::KnownEvent::AudioTranscriptDelta(
                    delta,
                ) => assert_eq!(delta.response_id.as_option().unwrap(), "target"),
                _ => {}
            }
        }
    }
    let mut decoder = Protocol::OpenAiResponses.stream_decoder(Limits::default());
    for value in &values[..3] {
        decoder
            .push(&raw(Protocol::OpenAiResponses, value.clone()))
            .unwrap();
    }
    assert!(
        decoder
            .push(&raw(Protocol::OpenAiResponses, values[5].clone()))
            .is_err()
    );
    let mut decoder = Protocol::OpenAiResponses.stream_decoder(Limits::default());
    decoder
        .push(&raw(Protocol::OpenAiResponses, values[0].clone()))
        .unwrap();
    assert!(decoder.push(&raw(Protocol::OpenAiResponses, json!({"type":"response.audio.delta","sequence_number":11,"response_id":"other","delta":"YQ=="}))).is_err());
}

#[test]
fn field_diagnostics_are_bounded_and_contain_no_extension_values() {
    let values = [
        (Protocol::OpenAiChat, {
            let mut body = chat(json!([]));
            body["private_extension"] = json!("secret-value");
            body
        }),
        (
            Protocol::OpenAiResponses,
            json!({"type":"response.created","sequence_number":0,"response":response(json!([])),"private_extension":"secret-value"}),
        ),
        (
            Protocol::AnthropicMessages,
            json!({"type":"ping","private_extension":"secret-value"}),
        ),
        (
            Protocol::Gemini,
            json!({"private_extension":"secret-value"}),
        ),
    ];
    for (source, value) in values {
        let events = source
            .stream_decoder(Limits::default())
            .push(&raw(source, value.clone()))
            .unwrap();
        let diagnostics = events
            .iter()
            .filter_map(|event| {
                if let Event::Diagnostic { diagnostic, .. } = event {
                    Some(diagnostic)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].path.ends_with("private_extension"));
        assert!(!format!("{diagnostics:?}").contains("secret-value"));
        let mut decoder = source.stream_decoder(Limits {
            buffered_bytes: 8,
            ..Limits::default()
        });
        assert!(decoder.push(&raw(source, value)).is_err());
    }
}

#[test]
fn native_progress_and_server_results_require_their_own_active_boundaries() {
    let mut decoder = Protocol::OpenAiResponses.stream_decoder(Limits::default());
    decoder
        .push(&raw(
            Protocol::OpenAiResponses,
            json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
        ))
        .unwrap();
    assert!(decoder.push(&raw(Protocol::OpenAiResponses, json!({"type":"response.code_interpreter_call_code.delta","sequence_number":1,"output_index":0,"item_id":"ci","delta":"code"}))).is_err());
    let mut decoder = Protocol::AnthropicMessages.stream_decoder(Limits::default());
    decoder.push(&raw(Protocol::AnthropicMessages, json!({"type":"message_start","message":{"id":"s","model":"m","role":"assistant","type":"message","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}))).unwrap();
    decoder.push(&raw(Protocol::AnthropicMessages, json!({"type":"content_block_start","index":0,"content_block":{"type":"bash_code_execution_tool_result","tool_use_id":"srv","content":{"stdout":"done","return_code":0}}}))).unwrap();
    assert!(decoder.push(&raw(Protocol::AnthropicMessages, json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}))).is_err());
}

#[test]
fn unsafe_native_actions_and_non_model_gemini_roles_are_not_forged_into_text_or_functions() {
    for item in [
        json!({"type":"computer_call","id":"c","action":{"type":"click","x":1,"y":2}}),
        json!({"type":"mcp_approval_request","id":"a","name":"tool","arguments":"{}","server_label":"private"}),
        json!({"type":"image_generation_call","id":"i","result":"YQ=="}),
    ] {
        for target in PROTOCOLS {
            let mut decoder = Protocol::OpenAiResponses.stream_decoder(Limits::default());
            let mut encoder = target
                .stream_encoder(Protocol::OpenAiResponses, &TARGET, Limits::default())
                .unwrap();
            for event in decoder.push(&raw(Protocol::OpenAiResponses, json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}))).unwrap() { encoder.push(&event).unwrap(); }
            let events = decoder.push(&raw(Protocol::OpenAiResponses, json!({"type":"response.output_item.done","sequence_number":1,"output_index":0,"item":item}))).unwrap();
            assert!(events.iter().any(|event| encoder.push(event).is_err()));
        }
    }
    let mut decoder = Protocol::Gemini.stream_decoder(Limits::default());
    assert!(
        decoder
            .push(&raw(
                Protocol::Gemini,
                json!({"candidates":[{"content":{"role":"user","parts":[{"text":"invalid"}]}}]})
            ))
            .is_err()
    );
}

#[test]
fn programmatic_tool_call_diagnostics_reject_before_any_target_tool_is_sent() {
    for target in PROTOCOLS {
        let source = Protocol::AnthropicMessages;
        let mut decoder = source.stream_decoder(Limits::default());
        let mut encoder = target
            .stream_encoder(source, &TARGET, Limits::default())
            .unwrap();
        for event in decoder.push(&raw(source, json!({"type":"message_start","message":{"id":"s","model":"m","role":"assistant","type":"message","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}))).unwrap() { encoder.push(&event).unwrap(); }
        let events = decoder.push(&raw(source, json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c","name":"tool","input":{},"caller":{"type":"code_execution_20250825","tool_id":"private-container-call"}}}))).unwrap();
        assert!(
            matches!(&events[0], Event::Diagnostic { diagnostic, .. } if diagnostic.reject && diagnostic.path == "content_block.caller")
        );
        let error = encoder.push(&events[0]).unwrap_err();
        assert!(!error.to_string().contains("private-container-call"));
    }
}
