//! 四套独立来源序列通过同一 trait 转为四个目标，再解码检查实际语义。
use crate::{
    adapter::protocol_codec::{ProtocolCodec, ResponseTarget},
    ir::{
        response::{FinishReason, Status},
        stream::{Event, Head, Key, Limits, Metadata, ToolHead},
        usage::Usage,
    },
    protocol::{Protocol, stream::Event as Raw},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PROTOCOLS: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];
const TARGET: ResponseTarget<'static> = ResponseTarget {
    id: "target",
    model: "target-model",
    created: 99,
};

/// JSON 仅作为测试的 HTTP 边界输入，不在编解码器内部中转。
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

fn chat(choices: Value) -> Value {
    json!({"id":"source","model":"source-model","created":1,"object":"chat.completion.chunk","choices":choices})
}
fn response(output: Value) -> Value {
    json!({"id":"source","model":"source-model","created_at":1,"object":"response","output":output})
}

/// 真实协议各自的文本、并行工具、部分参数及缓存统计，避免只验证编码器自己生成的载体。
fn sequence(protocol: Protocol) -> Vec<Raw> {
    let values = match protocol {
        Protocol::OpenAiChat => {
            let mut stats = chat(json!([]));
            stats["usage"] = json!({"prompt_tokens":8,"completion_tokens":4,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":3,"cache_write_tokens":2}});
            vec![
                chat(json!([{"index":0,"delta":{"role":"assistant","content":"你"}}])),
                chat(
                    json!([{"index":0,"delta":{"content":"好","tool_calls":[{"index":0,"id":"a","function":{"name":"get_","arguments":"{\"x\":"}},{"index":1,"id":"b","function":{"name":"other","arguments":"{}"}}]}}]),
                ),
                chat(
                    json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"value","arguments":"1}"}}]},"finish_reason":"tool_calls"}]),
                ),
                stats,
            ]
        }
        Protocol::AnthropicMessages => vec![
            json!({"type":"message_start","message":{"id":"source","model":"source-model","type":"message","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":3,"output_tokens":0,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"a","name":"get_value","input":{}}}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"b","name":"other","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"1}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ],
        Protocol::OpenAiResponses => {
            let message = json!({"id":"m1","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"你好","annotations":[]}]});
            let call1 = json!({"type":"function_call","id":"fc1","call_id":"a","name":"get_value","arguments":"{\"x\":1}"});
            let call2 = json!({"type":"function_call","id":"fc2","call_id":"b","name":"other","arguments":"{}"});
            let mut final_response =
                response(json!([message.clone(), call1.clone(), call2.clone()]));
            final_response["status"] = json!("completed");
            final_response["usage"] = json!({"input_tokens":8,"output_tokens":4,"total_tokens":12,"input_tokens_details":{"cached_tokens":3,"cache_write_tokens":2}});
            vec![
                json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
                json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":"m1","type":"message","role":"assistant","status":"in_progress","content":[]}}),
                json!({"type":"response.content_part.added","sequence_number":2,"output_index":0,"content_index":0,"item_id":"m1","part":{"type":"output_text","text":"","annotations":[]}}),
                json!({"type":"response.output_text.delta","sequence_number":3,"output_index":0,"content_index":0,"item_id":"m1","delta":"你"}),
                json!({"type":"response.output_text.delta","sequence_number":4,"output_index":0,"content_index":0,"item_id":"m1","delta":"好"}),
                json!({"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":message}),
                json!({"type":"response.output_item.added","sequence_number":6,"output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"a","name":"get_value","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","sequence_number":7,"output_index":1,"item_id":"fc1","delta":"{\"x\":"}),
                json!({"type":"response.output_item.added","sequence_number":8,"output_index":2,"item":{"type":"function_call","id":"fc2","call_id":"b","name":"other","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","sequence_number":9,"output_index":2,"item_id":"fc2","delta":"{}"}),
                json!({"type":"response.function_call_arguments.delta","sequence_number":10,"output_index":1,"item_id":"fc1","delta":"1}"}),
                json!({"type":"response.output_item.done","sequence_number":11,"output_index":1,"item":call1}),
                json!({"type":"response.output_item.done","sequence_number":12,"output_index":2,"item":call2}),
                json!({"type":"response.completed","sequence_number":13,"response":final_response}),
            ]
        }
        Protocol::Gemini => vec![
            json!({"responseId":"source","modelVersion":"source-model","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"你"}]}}]}),
            json!({"responseId":"source","modelVersion":"source-model","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"好"},{"functionCall":{"id":"a","name":"get_value","args":{"x":1}},"thoughtSignature":"private-signature"},{"functionCall":{"id":"b","name":"other","args":{}}}]}}]}),
            json!({"responseId":"source","modelVersion":"source-model","candidates":[{"index":0,"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":4,"totalTokenCount":12,"cachedContentTokenCount":3}}),
        ],
    };
    let mut events = values
        .into_iter()
        .map(|value| raw(protocol, value))
        .collect::<Vec<_>>();
    events.push(Raw::End(protocol));
    events
}

/// 捕获实际交付增量，完成快照不能使文字和参数重复追加。
#[derive(Default)]
struct Observed {
    text: String,
    tools: BTreeMap<Key, (String, String)>,
    ids: BTreeMap<Key, String>,
    signatures: Vec<String>,
}
impl Observed {
    fn accept(&mut self, event: &Event) {
        match event {
            Event::TextDelta { text, .. } => self.text.push_str(text),
            Event::PartStart {
                key,
                head: Head::Tool(head),
            } => {
                if let Some(id) = &head.id {
                    self.ids.insert(*key, id.clone());
                }
                self.tools
                    .insert(*key, (head.name.clone().unwrap_or_default(), String::new()));
            }
            Event::ToolDelta {
                key,
                id,
                name,
                arguments,
                ..
            } => {
                if let Some(id) = id {
                    self.ids.insert(*key, id.clone());
                }
                let tool = self.tools.get_mut(key).unwrap();
                if let Some(name) = name {
                    tool.0.push_str(name);
                }
                if let Some(arguments) = arguments {
                    tool.1.push_str(arguments);
                }
            }
            Event::Signature { data, .. } => self.signatures.push(data.clone()),
            _ => {}
        }
    }
}

#[test]
fn sixteen_directions_deliver_text_tools_and_cache_usage_without_replaying_snapshots() {
    for source in PROTOCOLS {
        for target in PROTOCOLS {
            let mut decoder = source.stream_decoder(Limits::default());
            let mut encoder = target
                .stream_encoder(source, &TARGET, Limits::default())
                .unwrap();
            let mut verify = target.stream_decoder(Limits::default());
            let mut observed = Observed::default();
            let mut output = Vec::new();
            for raw in sequence(source) {
                for event in decoder.push(&raw).unwrap() {
                    let converted = encoder
                        .push(&event)
                        .unwrap_or_else(|error| panic!("{source:?} -> {target:?}: {error}"));
                    let mut immediate_text = String::new();
                    for frame in converted.body {
                        assert_eq!(frame.protocol(), target);
                        for returned in verify.push(&frame).unwrap_or_else(|error| {
                            panic!("{source:?} -> {target:?}: {error}, frame={frame:?}")
                        }) {
                            if let Event::TextDelta { text, .. } = &returned {
                                immediate_text.push_str(text);
                            }
                            observed.accept(&returned);
                        }
                        output.push(frame);
                    }
                    if let Event::TextDelta { text, .. } = &event {
                        assert_eq!(
                            immediate_text, *text,
                            "{source:?} -> {target:?} 必须立即交付文本"
                        );
                    }
                }
            }
            assert_eq!(observed.text, "你好", "{source:?} -> {target:?}");
            let mut ids = observed.ids.into_values().collect::<Vec<_>>();
            ids.sort();
            assert_eq!(
                ids,
                ["a", "b"],
                "{source:?} -> {target:?} 应保留已经报告的调用 ID"
            );
            let mut tools = observed
                .tools
                .into_values()
                .map(|(name, args)| (name, serde_json::from_str::<Value>(&args).unwrap()))
                .collect::<Vec<_>>();
            tools.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(
                tools,
                vec![
                    ("get_value".into(), json!({"x":1})),
                    ("other".into(), json!({}))
                ]
            );
            let usage = verify.state().usage().snapshot().unwrap();
            assert_eq!(
                (usage.input_tokens, usage.output_tokens, usage.total_tokens),
                (Some(8), Some(4), Some(12))
            );
            assert_eq!(usage.cache.read_input_tokens, Some(3));
            assert_eq!(
                usage.cache.write_input_tokens,
                if target == Protocol::Gemini || source == Protocol::Gemini {
                    None
                } else {
                    Some(2)
                }
            );
            assert_eq!(verify.state().ended(), Some(Status::Completed));
            let metadata = verify.state().metadata().unwrap();
            assert_eq!(metadata.id.as_deref(), Some(TARGET.id));
            assert_eq!(metadata.model.as_deref(), Some(TARGET.model));
            assert_eq!(encoder.buffered_bytes(), 0);
            assert!(
                encoder
                    .push(&Event::End(Status::Completed))
                    .unwrap()
                    .body
                    .is_empty()
            );
            if source == Protocol::Gemini && target == Protocol::Gemini {
                assert_eq!(observed.signatures, ["private-signature"]);
            }
            if target == Protocol::OpenAiResponses {
                let numbers = output
                    .iter()
                    .filter_map(|frame| match frame {
                        Raw::Responses(event) => match event.as_ref() {
                            crate::protocol::responses::response::Event::Known(event) => {
                                Some(event.sequence_number())
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert!(numbers.windows(2).all(|pair| pair[0] < pair[1]));
            }
        }
    }
}

fn key(candidate: u64, item: u64) -> Key {
    Key {
        candidate,
        item,
        part: 0,
    }
}
fn usage() -> Event {
    Event::Usage(Box::new(Usage {
        input_tokens: Some(8),
        output_tokens: Some(4),
        total_tokens: Some(12),
        ..Default::default()
    }))
}

#[test]
fn only_responses_retains_text_and_its_snapshot_shares_the_tool_budget() {
    for protocol in PROTOCOLS {
        let mut encoder = protocol
            .stream_encoder(
                Protocol::OpenAiChat,
                &TARGET,
                Limits {
                    buffered_bytes: 64,
                    entries: 16,
                },
            )
            .unwrap();
        for event in [
            Event::Start(Metadata::default()),
            Event::CandidateStart(0),
            Event::PartStart {
                key: key(0, 0),
                head: Head::Text,
            },
        ] {
            encoder.push(&event).unwrap();
        }
        let bytes = encoder.buffered_bytes();
        encoder
            .push(&Event::TextDelta {
                key: key(0, 0),
                text: "x".repeat(40),
            })
            .unwrap();
        assert_eq!(
            encoder.buffered_bytes(),
            if protocol == Protocol::OpenAiResponses {
                bytes + 40
            } else {
                0
            }
        );
        encoder
            .push(&Event::PartStart {
                key: key(0, 1),
                head: Head::Tool(ToolHead {
                    name: Some("f".into()),
                    ..Default::default()
                }),
            })
            .unwrap();
        let result = encoder.push(&Event::ToolDelta {
            key: key(0, 1),
            id: None,
            name: None,
            arguments: Some("x".repeat(30)),
        });
        assert_eq!(result.is_err(), protocol == Protocol::OpenAiResponses);
        if protocol == Protocol::OpenAiResponses {
            assert!(encoder.push(&usage()).is_err());
        }
    }
}

#[test]
fn structured_targets_reject_truncated_tool_arguments_and_missing_actual_messages_usage() {
    for target in PROTOCOLS {
        let mut encoder = target
            .stream_encoder(Protocol::OpenAiChat, &TARGET, Limits::default())
            .unwrap();
        for event in [
            Event::Start(Metadata::default()),
            Event::CandidateStart(0),
            Event::PartStart {
                key: key(0, 0),
                head: Head::Tool(ToolHead {
                    name: Some("f".into()),
                    ..Default::default()
                }),
            },
            Event::ToolDelta {
                key: key(0, 0),
                id: None,
                name: None,
                arguments: Some("{".into()),
            },
        ] {
            encoder.push(&event).unwrap();
        }
        assert_eq!(
            encoder.push(&Event::PartEnd(key(0, 0))).is_err(),
            matches!(target, Protocol::Gemini | Protocol::AnthropicMessages)
        );
    }
    let mut encoder = Protocol::AnthropicMessages
        .stream_encoder(Protocol::OpenAiChat, &TARGET, Limits::default())
        .unwrap();
    encoder.push(&Event::Start(Metadata::default())).unwrap();
    assert!(encoder.state().usage().snapshot().is_none());
    encoder.push(&Event::CandidateStart(0)).unwrap();
    encoder
        .push(&Event::CandidateEnd {
            index: 0,
            reason: FinishReason::Stop,
        })
        .unwrap();
    assert!(encoder.push(&Event::End(Status::Completed)).is_err());
}

#[test]
fn single_candidate_targets_do_not_merge_interleaved_answers() {
    for target in [Protocol::AnthropicMessages, Protocol::OpenAiResponses] {
        let mut encoder = target
            .stream_encoder(Protocol::OpenAiChat, &TARGET, Limits::default())
            .unwrap();
        encoder.push(&Event::Start(Metadata::default())).unwrap();
        let discarded = encoder.push(&Event::CandidateStart(1)).unwrap();
        assert!(discarded.body.is_empty());
        assert_eq!(discarded.warnings[0].path, "candidates");
        for event in [
            Event::PartStart {
                key: key(1, 0),
                head: Head::Text,
            },
            Event::TextDelta {
                key: key(1, 0),
                text: "private answer".into(),
            },
            Event::PartEnd(key(1, 0)),
            Event::CandidateEnd {
                index: 1,
                reason: FinishReason::Stop,
            },
        ] {
            assert!(encoder.push(&event).unwrap().body.is_empty());
        }
        assert!(encoder.push(&Event::End(Status::Completed)).is_err());
    }
}

#[test]
fn native_events_are_not_replayed_after_public_fields_have_been_projected() {
    for target in PROTOCOLS {
        let mut encoder = target
            .stream_encoder(Protocol::OpenAiChat, &TARGET, Limits::default())
            .unwrap();
        encoder.push(&Event::Start(Metadata::default())).unwrap();
        assert!(encoder.push(&Event::Native(Box::new(raw(Protocol::OpenAiChat, chat(json!([{"index":0,"delta":{"content":"already delivered","audio":{"data":"YQ=="}}}])))))).is_err());
        assert!(encoder.push(&usage()).is_err());
    }
}

/// IR 直接编码后再从目标事件读取，检查目标协议的真实生命周期。
fn convert(
    source: Protocol,
    target: Protocol,
    events: Vec<Event>,
) -> (
    Vec<Event>,
    Vec<crate::adapter::protocol_codec::ConversionWarning>,
) {
    let mut encoder = target
        .stream_encoder(source, &TARGET, Limits::default())
        .unwrap();
    let mut decoder = target.stream_decoder(Limits::default());
    let mut output = Vec::new();
    let mut warnings = Vec::new();
    for event in events {
        let converted = encoder.push(&event).unwrap();
        warnings.extend(converted.warnings);
        for frame in converted.body {
            output.extend(
                decoder
                    .push(&frame)
                    .unwrap_or_else(|error| panic!("{target:?}: {error}, frame={frame:?}")),
            );
        }
    }
    (output, warnings)
}

#[test]
fn length_and_filter_endings_are_not_encoded_as_natural_stops() {
    for reason in [FinishReason::Length, FinishReason::Filtered] {
        for target in PROTOCOLS {
            let (events, warnings) = convert(
                Protocol::OpenAiChat,
                target,
                vec![
                    Event::Start(Metadata::default()),
                    Event::CandidateStart(0),
                    Event::PartStart {
                        key: key(0, 0),
                        head: Head::Text,
                    },
                    Event::TextDelta {
                        key: key(0, 0),
                        text: "partial".into(),
                    },
                    Event::PartEnd(key(0, 0)),
                    usage(),
                    Event::CandidateEnd { index: 0, reason },
                    Event::End(Status::Completed),
                ],
            );
            let expected =
                if target == Protocol::AnthropicMessages && reason == FinishReason::Filtered {
                    FinishReason::Refusal
                } else {
                    reason
                };
            assert!(events.iter().any(
                |event| matches!(event, Event::CandidateEnd { reason, .. } if *reason == expected)
            ));
            let status = if target == Protocol::OpenAiResponses {
                Status::Incomplete
            } else {
                Status::Completed
            };
            assert!(events.contains(&Event::End(status)));
            if expected != reason {
                assert!(
                    warnings
                        .iter()
                        .any(|warning| warning.path == "finish_reason")
                );
            }
        }
    }
}

#[test]
fn reasoning_and_signatures_keep_their_semantics_without_forging_target_signatures() {
    for source in [Protocol::AnthropicMessages, Protocol::Gemini] {
        for target in PROTOCOLS {
            let (events, warnings) = convert(
                source,
                target,
                vec![
                    Event::Start(Metadata::default()),
                    Event::CandidateStart(0),
                    Event::PartStart {
                        key: key(0, 0),
                        head: Head::Reasoning,
                    },
                    Event::TextDelta {
                        key: key(0, 0),
                        text: "plan".into(),
                    },
                    Event::Signature {
                        key: key(0, 0),
                        protocol: source,
                        data: "private signature".into(),
                    },
                    Event::PartEnd(key(0, 0)),
                    Event::PartStart {
                        key: key(0, 1),
                        head: Head::Text,
                    },
                    Event::TextDelta {
                        key: key(0, 1),
                        text: "answer".into(),
                    },
                    Event::PartEnd(key(0, 1)),
                    usage(),
                    Event::CandidateEnd {
                        index: 0,
                        reason: FinishReason::Stop,
                    },
                    Event::End(Status::Completed),
                ],
            );
            let texts = events
                .iter()
                .filter_map(|event| match event {
                    Event::TextDelta { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                texts,
                if target == Protocol::OpenAiChat {
                    vec!["answer"]
                } else {
                    vec!["plan", "answer"]
                }
            );
            let reasoning = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::PartStart {
                            head: Head::Reasoning,
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(
                reasoning,
                usize::from(
                    target != Protocol::OpenAiChat
                        && !(target == Protocol::AnthropicMessages && source == Protocol::Gemini)
                )
            );
            let signatures = events
                .iter()
                .filter(|event| matches!(event, Event::Signature { .. }))
                .count();
            assert_eq!(signatures, usize::from(source == target));
            if source != target {
                assert!(warnings.iter().any(|warning| warning.path == "signature"));
            }
        }
    }
}

#[test]
fn responses_late_citations_are_sent_once_before_final_item_snapshot() {
    let annotation = json!({"type":"url_citation","url":"https://example.com","title":"source","start_index":0,"end_index":4});
    let (events, _) = convert(
        Protocol::OpenAiResponses,
        Protocol::OpenAiResponses,
        vec![
            Event::Start(Metadata::default()),
            Event::CandidateStart(0),
            Event::PartStart {
                key: key(0, 0),
                head: Head::Text,
            },
            Event::TextDelta {
                key: key(0, 0),
                text: "text".into(),
            },
            Event::PartEnd(key(0, 0)),
            Event::Annotation {
                key: key(0, 0),
                annotation: annotation.clone(),
            },
            usage(),
            Event::CandidateEnd {
                index: 0,
                reason: FinishReason::Stop,
            },
            Event::End(Status::Completed),
        ],
    );
    let annotations = events
        .iter()
        .filter_map(|event| match event {
            Event::Annotation { annotation, .. } => Some(annotation),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(annotations, [&annotation]);
    let text = events
        .iter()
        .filter_map(|event| match event {
            Event::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(text, "text");
}

#[test]
fn repeated_cache_snapshots_keep_totals_and_report_only_unsupported_breakdowns() {
    let mut snapshot = Usage {
        input_tokens: Some(100),
        output_tokens: Some(3),
        total_tokens: Some(103),
        ..Default::default()
    };
    snapshot.cache.read_input_tokens = Some(50);
    snapshot.cache.write_input_tokens = Some(30);
    snapshot.cache.write_short_input_tokens = Some(10);
    snapshot.cache.write_long_input_tokens = Some(20);
    for target in PROTOCOLS {
        let (events, warnings) = convert(
            Protocol::AnthropicMessages,
            target,
            vec![
                Event::Start(Metadata::default()),
                Event::CandidateStart(0),
                Event::Usage(Box::new(snapshot.clone())),
                Event::Usage(Box::new(snapshot.clone())),
                Event::CandidateEnd {
                    index: 0,
                    reason: FinishReason::Stop,
                },
                Event::End(Status::Completed),
            ],
        );
        let mut usage = crate::ir::stream::UsageState::default();
        for event in events {
            if let Event::Usage(snapshot) = event {
                usage.update(&snapshot);
            }
        }
        let result = usage.snapshot().unwrap();
        assert_eq!(
            (
                result.input_tokens,
                result.output_tokens,
                result.total_tokens
            ),
            (Some(100), Some(3), Some(103))
        );
        assert_eq!(result.cache.read_input_tokens, Some(50));
        if target == Protocol::AnthropicMessages {
            assert_eq!(
                (
                    result.cache.write_short_input_tokens,
                    result.cache.write_long_input_tokens
                ),
                (Some(10), Some(20))
            );
        } else {
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.path == "usage.cache.write_long_input_tokens")
            );
        }
        assert_eq!(
            result.cache.write_input_tokens,
            if target == Protocol::Gemini {
                None
            } else {
                Some(30)
            }
        );
    }
}

#[test]
fn failures_release_state_and_never_expose_private_provider_messages_or_success_events() {
    for target in PROTOCOLS {
        let mut encoder = target
            .stream_encoder(Protocol::AnthropicMessages, &TARGET, Limits::default())
            .unwrap();
        for event in [
            Event::Start(Metadata::default()),
            Event::CandidateStart(0),
            Event::PartStart {
                key: key(0, 0),
                head: Head::Tool(ToolHead::default()),
            },
            Event::ToolDelta {
                key: key(0, 0),
                id: None,
                name: None,
                arguments: Some("{".into()),
            },
        ] {
            encoder.push(&event).unwrap();
        }
        let failure = Event::Failure(crate::ir::response::Failure {
            code: "private-provider-code".into(),
            message: "private request body and token".into(),
        });
        let result = encoder.push(&failure);
        if matches!(target, Protocol::OpenAiChat | Protocol::Gemini) {
            assert!(matches!(result, Err(crate::adapter::Error::FailedResponse)));
        } else {
            let mut frames = result.unwrap().body;
            frames.extend(encoder.push(&Event::End(Status::Failed)).unwrap().body);
            let text = serde_json::to_string(&frames).unwrap();
            assert!(!text.contains("private-provider-code"));
            assert!(!text.contains("private request body"));
            assert!(!text.contains("message_stop"));
            assert!(!text.contains("response.completed"));
            assert_eq!(encoder.buffered_bytes(), 0);
        }
    }
}

#[test]
fn unknown_cross_events_only_warn_with_static_paths_and_same_protocol_requires_passthrough() {
    let event = Event::Unknown {
        protocol: Protocol::AnthropicMessages,
        event: crate::protocol::stream::UnknownEvent {
            r#type: "private-extension-name".into(),
            fields: serde_json::Map::from_iter([("body".into(), json!("private contents"))]),
        },
    };
    for target in PROTOCOLS {
        let mut encoder = target
            .stream_encoder(Protocol::AnthropicMessages, &TARGET, Limits::default())
            .unwrap();
        let converted = encoder.push(&event);
        if target == Protocol::AnthropicMessages {
            assert!(converted.is_err());
        } else {
            let converted = converted.unwrap();
            assert!(converted.body.is_empty());
            assert_eq!(converted.warnings[0].path, "stream.unknown");
            let text = serde_json::to_string(&converted.warnings).unwrap();
            assert!(!text.contains("private"));
        }
    }
}

#[test]
fn messages_tools_without_parameter_deltas_keep_empty_objects_and_explicit_zero_usage() {
    let source = Protocol::AnthropicMessages;
    let source_frames = [
        json!({"type":"message_start","message":{"id":"source","model":"source-model","type":"message","role":"assistant","content":[],"usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call","name":"f","input":{}}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":0}}),
        json!({"type":"message_stop"}),
    ];
    let mut decoder = source.stream_decoder(Limits::default());
    let mut input = Vec::new();
    for frame in source_frames {
        input.extend(decoder.push(&raw(source, frame)).unwrap());
    }
    for target in PROTOCOLS {
        let (events, _) = convert(source, target, input.clone());
        let mut observed = Observed::default();
        let mut stats = crate::ir::stream::UsageState::default();
        for event in events {
            observed.accept(&event);
            if let Event::Usage(usage) = event {
                stats.update(&usage);
            }
        }
        let tool = observed.tools.into_values().next().unwrap();
        assert_eq!((tool.0.as_str(), tool.1.as_str()), ("f", "{}"));
        let stats = stats.snapshot().unwrap();
        assert_eq!(
            (stats.input_tokens, stats.output_tokens, stats.total_tokens),
            (Some(2), Some(0), Some(2))
        );
        assert_eq!(stats.cache.read_input_tokens, None);
        assert_eq!(stats.cache.write_input_tokens, None);
    }
}

#[test]
fn custom_text_tools_only_use_the_responses_custom_tool_carrier() {
    let input = vec![
        Event::Start(Metadata::default()),
        Event::CandidateStart(0),
        Event::PartStart {
            key: key(0, 0),
            head: Head::Tool(ToolHead {
                id: Some("call".into()),
                name: Some("script".into()),
                text_input: true,
            }),
        },
        Event::ToolDelta {
            key: key(0, 0),
            id: None,
            name: None,
            arguments: Some("print(1)".into()),
        },
        Event::PartEnd(key(0, 0)),
        usage(),
        Event::CandidateEnd {
            index: 0,
            reason: FinishReason::ToolCall,
        },
        Event::End(Status::Completed),
    ];
    let (events, _) = convert(
        Protocol::OpenAiResponses,
        Protocol::OpenAiResponses,
        input.clone(),
    );
    assert!(events.iter().any(
        |event| matches!(event, Event::PartStart { head: Head::Tool(head), .. } if head.text_input)
    ));
    let mut observed = Observed::default();
    for event in events {
        observed.accept(&event);
    }
    assert_eq!(
        observed.tools.into_values().next().unwrap(),
        ("script".into(), "print(1)".into())
    );
    for target in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::Gemini,
    ] {
        let mut encoder = target
            .stream_encoder(Protocol::OpenAiResponses, &TARGET, Limits::default())
            .unwrap();
        let mut refused = false;
        for event in &input {
            if encoder.push(event).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused, "{target:?} 不能把文本工具输入伪装成 JSON 函数参数");
    }
}

#[test]
fn gemini_media_uses_typed_parts_and_other_targets_refuse_instead_of_inventing_text() {
    let source = Protocol::Gemini;
    let mut decoder = source.stream_decoder(Limits::default());
    let mut input = decoder.push(&raw(source, json!({"responseId":"source","modelVersion":"m","candidates":[{"index":0,"content":{"role":"model","parts":[{"inlineData":{"mimeType":"image/png","data":"YQ=="}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":1,"totalTokenCount":3}}))).unwrap();
    input.extend(decoder.push(&Raw::End(source)).unwrap());
    let (events, _) = convert(source, source, input.clone());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::Media { .. }))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::TextDelta { .. }))
    );
    for target in [
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        Protocol::OpenAiResponses,
    ] {
        let mut encoder = target
            .stream_encoder(source, &TARGET, Limits::default())
            .unwrap();
        let mut refused = false;
        for event in &input {
            if encoder.push(event).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused);
    }
}
