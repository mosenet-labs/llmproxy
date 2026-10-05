use super::incremental::Reply;
use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::protocol::Protocol;
use serde_json::{Value, json};

fn packet(value: Value) -> Vec<u8> {
    format!("data: {}\n\n", serde_json::to_string(&value).unwrap()).into_bytes()
}

#[test]
fn tool_only_streams_display_args_and_usage_in_four_protocols() {
    for (protocol, values) in [
        (
            Protocol::OpenAiChat,
            vec![
                json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},"finish_reason":"tool_calls"}]}),
                json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":4}}}),
            ],
        ),
        (
            Protocol::OpenAiResponses,
            vec![
                json!({"type":"response.created","sequence_number":0,"response":{"id":"r","model":"m","created_at":1,"object":"response","output":[]}}),
                json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"function_call","id":"f","call_id":"a","name":"lookup","arguments":""}}),
                json!({"type":"response.output_item.done","sequence_number":2,"output_index":0,"item":{"type":"function_call","id":"f","call_id":"a","name":"lookup","arguments":"{\"q\":1}"}}),
                json!({"type":"response.completed","sequence_number":3,"response":{"id":"r","model":"m","created_at":1,"object":"response","status":"completed","output":[{"type":"function_call","id":"f","call_id":"a","name":"lookup","arguments":"{\"q\":1}"}],"usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}}}}),
            ],
        ),
        (
            Protocol::AnthropicMessages,
            vec![
                json!({"type":"message_start","message":{"id":"m","type":"message","model":"m","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":6,"cache_read_input_tokens":4,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"lookup","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":1}"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
                json!({"type":"message_stop"}),
            ],
        ),
        (
            Protocol::Gemini,
            vec![
                json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":"a","name":"lookup","args":{"q":1}},"thoughtSignature":"private"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13,"cachedContentTokenCount":4}}),
            ],
        ),
    ] {
        let mut reply = Reply::new(protocol);
        let mut updates = 0;
        for value in values {
            for bytes in packet(value).chunks(2) {
                reply.push(bytes, &mut |_| updates += 1).unwrap();
            }
        }
        if protocol == Protocol::OpenAiChat {
            reply
                .push(b"data: [DONE]\n\n", &mut |_| updates += 1)
                .unwrap();
        }
        let reply = reply.finish().unwrap();
        assert!(updates > 0);
        assert!(reply.content.is_empty());
        assert_eq!(reply.parts.len(), 1);
        assert_eq!(reply.parts[0].title, "工具调用 · lookup");
        assert_eq!(
            serde_json::from_str::<Value>(&reply.parts[0].text).unwrap(),
            json!({"q":1})
        );
        let usage = reply.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.total_tokens, Some(13));
        assert_eq!(usage.cache.read_input_tokens, Some(4));
        assert!(!format!("{:?}", reply.parts).contains("private"));
    }
}

#[test]
fn partial_text_updates_immediately_and_truncation_does_not_complete() {
    let mut reply = Reply::new(Protocol::OpenAiChat);
    let mut text = String::new();
    reply.push(&packet(json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"你好","reasoning_content":"分析"}}]})),&mut |r|text=r.content.clone()).unwrap();
    assert_eq!(text, "你好");
    assert!(reply.finish().is_err());
    let mut reply = Reply::new(Protocol::OpenAiChat);
    assert!(
        reply
            .push(
                b"data: {\"error\":{\"message\":\"private-provider-detail\"}}\n\n",
                &mut |_| {}
            )
            .unwrap_err()
            .contains("生成失败")
    );
}

/// 独立编码的两段音频包含 padding，不能把原始 Base64 字符串直接拼接。
fn audio_events(protocol: Protocol, bytes: &[u8]) -> (Vec<Value>, Vec<Value>) {
    let (first, second) = bytes.split_at(bytes.len().min(1));
    let first = STANDARD.encode(first);
    let second = STANDARD.encode(second);
    if protocol == Protocol::OpenAiChat {
        let chunk = |delta: Value, finish: Value| json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        (
            vec![
                chunk(
                    json!({"role":"assistant","audio":{"id":"private-audio-id","data":first,"transcript":"你好，"}}),
                    Value::Null,
                ),
                chunk(
                    json!({"audio":{"data":second,"transcript":"音频测试。"}}),
                    Value::Null,
                ),
            ],
            vec![
                chunk(json!({}), json!("stop")),
                chunk(json!({"audio":{"expires_at":123}}), Value::Null),
            ],
        )
    } else {
        (
            vec![
                json!({"type":"response.created","sequence_number":0,"response":{"id":"r","model":"m","created_at":1,"object":"response","output":[]}}),
                json!({"type":"response.audio.transcript.delta","sequence_number":1,"response_id":"r","delta":"你好，"}),
                json!({"type":"response.audio.delta","sequence_number":2,"response_id":"r","delta":first}),
                json!({"type":"response.audio.delta","sequence_number":3,"response_id":"r","delta":second}),
                json!({"type":"response.audio.transcript.delta","sequence_number":4,"response_id":"r","delta":"音频测试。"}),
            ],
            vec![
                json!({"type":"response.audio.transcript.done","sequence_number":5,"response_id":"r"}),
                json!({"type":"response.audio.done","sequence_number":6,"response_id":"r"}),
                json!({"type":"response.completed","sequence_number":7,"response":{"id":"r","model":"m","created_at":1,"object":"response","status":"completed","output":[]}}),
            ],
        )
    }
}

#[test]
fn native_audio_joins_decoded_chunks_and_streams_transcripts_without_history() {
    for protocol in [Protocol::OpenAiChat, Protocol::OpenAiResponses] {
        for bytes in [
            b"abc".as_slice(),
            b"RIFF\x04\0\0\0WAVEdata".as_slice(),
            b"".as_slice(),
        ] {
            let (start, end) = audio_events(protocol, bytes);
            let mut reply = Reply::new(protocol);
            let mut transcript = String::new();
            for value in start {
                // 验证 UTF-8 及 HTTP 分块不影响音频和转录的逐帧投影。
                for bytes in packet(value).chunks(2) {
                    reply
                        .push(bytes, &mut |r| {
                            assert!(r.content.is_empty());
                            assert!(r.parts.iter().all(|part| part.media.is_none()));
                            if let Some(part) = r.parts.iter().find(|part| part.title == "音频转录")
                            {
                                transcript = part.text.clone();
                            }
                        })
                        .unwrap();
                }
            }
            assert_eq!(transcript, "你好，音频测试。");
            for value in end {
                reply.push(&packet(value), &mut |_| {}).unwrap();
            }
            if protocol == Protocol::OpenAiChat {
                reply.push(b"data: [DONE]\n\n", &mut |_| {}).unwrap();
            }
            let reply = reply.finish().unwrap();
            assert!(reply.content.is_empty());
            assert_eq!(
                reply
                    .parts
                    .iter()
                    .filter(|part| part.title == "音频转录")
                    .count(),
                1
            );
            let media = reply.parts.iter().find_map(|part| part.media.as_ref());
            if bytes.is_empty() {
                assert!(media.is_none());
            } else {
                let media = media.unwrap();
                assert_eq!(
                    STANDARD
                        .decode(media.uri.split_once(',').unwrap().1)
                        .unwrap(),
                    bytes
                );
                assert_eq!(media.preview, bytes.starts_with(b"RIFF"));
            }
            assert!(!format!("{:?}", reply.parts).contains("private-audio-id"));
        }
    }
}

#[test]
fn audio_and_transcript_require_their_own_terminal_events() {
    for protocol in [Protocol::OpenAiChat, Protocol::OpenAiResponses] {
        let (start, end) = audio_events(protocol, b"abc");
        let mut reply = Reply::new(protocol);
        for value in start.clone() {
            reply.push(&packet(value), &mut |_| {}).unwrap();
        }
        assert!(reply.finish().is_err(), "{protocol:?}");
        if protocol == Protocol::OpenAiResponses {
            for missing in ["response.audio.done", "response.audio.transcript.done"] {
                let mut reply = Reply::new(protocol);
                for value in start
                    .iter()
                    .chain(end.iter())
                    .filter(|value| value["type"] != missing)
                {
                    if reply.push(&packet(value.clone()), &mut |_| {}).is_err() {
                        break;
                    }
                }
                assert!(reply.finish().is_err(), "{missing}");
            }
        }
    }
}

#[test]
fn native_audio_rejects_invalid_encoding_and_accumulated_oversize() {
    for data in [
        "not-base64!".to_owned(),
        STANDARD.encode(vec![0; 64 * 1024]),
    ] {
        let mut reply = Reply::new(Protocol::OpenAiResponses);
        reply
            .push(
                &packet(audio_events(Protocol::OpenAiResponses, b"abc").0.remove(0)),
                &mut |_| {},
            )
            .unwrap();
        let mut error = None;
        for sequence in 1..=4 {
            let value =
                json!({"type":"response.audio.delta","sequence_number":sequence,"delta":data});
            if let Err(failure) = reply.push(&packet(value), &mut |_| {}) {
                error = Some(failure);
                break;
            }
        }
        assert_eq!(
            error.as_deref(),
            Some(if data == "not-base64!" {
                "上游音频数据无效"
            } else {
                "上游回复过长"
            })
        );
    }
}
