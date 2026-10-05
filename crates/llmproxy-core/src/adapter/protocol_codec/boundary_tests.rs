//! 独立边界夹具：同协议保留，跨协议诊断不泄露字段值。
use super::{
    RequestTarget, ResponseTarget,
    cross_tests::{request, response},
    json_test_support::JsonCodec,
};
use crate::protocol::Protocol;
use serde_json::{Value, json};

const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];
const TARGET: RequestTarget<'static> = RequestTarget {
    model: "target",
    max_output_tokens: Some(64),
};

#[test]
fn nested_extensions_have_exact_paths_and_never_leak_values() {
    let secret = "private-boundary-value";
    let cases: Vec<(Protocol, &str, Value, Vec<&str>)> = vec![
        (
            Protocol::OpenAiChat,
            "tools",
            json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object"},"vendor":secret}}]),
            vec!["tools[0].function.vendor"],
        ),
        (
            Protocol::OpenAiChat,
            "tool_choice",
            json!({"type":"function","function":{"name":"lookup","vendor":secret}}),
            vec!["tool_choice.function.vendor"],
        ),
        (
            Protocol::OpenAiChat,
            "tool_choice",
            json!({"type":"allowed_tools","allowed_tools":{"mode":"auto","tools":[{"type":"function","vendor":secret,"function":{"name":"lookup","vendor":secret}}]}}),
            vec![
                "tool_choice.allowed_tools.tools[0].vendor",
                "tool_choice.allowed_tools.tools[0].function.vendor",
            ],
        ),
        (
            Protocol::OpenAiChat,
            "response_format",
            json!({"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object"},"vendor":secret}}),
            vec!["response_format.json_schema.vendor"],
        ),
        (
            Protocol::OpenAiResponses,
            "tool_choice",
            json!({"type":"function","name":"lookup","vendor":secret}),
            vec!["tool_choice.vendor"],
        ),
        (
            Protocol::OpenAiResponses,
            "tool_choice",
            json!({"type":"allowed_tools","mode":"auto","vendor":secret,"tools":[{"type":"function","name":"lookup","vendor":secret}]}),
            vec!["tool_choice.vendor", "tool_choice.tools[0].vendor"],
        ),
        (
            Protocol::OpenAiResponses,
            "text",
            json!({"format":{"type":"json_schema","name":"answer","schema":{"type":"object"},"vendor":secret},"vendor":secret}),
            vec!["text.vendor", "text.format.vendor"],
        ),
        (
            Protocol::OpenAiResponses,
            "reasoning",
            json!({"vendor":secret}),
            vec!["reasoning.vendor"],
        ),
        (
            Protocol::AnthropicMessages,
            "thinking",
            json!({"type":"disabled","vendor":secret}),
            vec!["thinking.vendor"],
        ),
        (
            Protocol::AnthropicMessages,
            "output_config",
            json!({"format":{"type":"json_schema","schema":{"type":"object"},"vendor":secret},"vendor":secret}),
            vec!["output_config.vendor", "output_config.format.vendor"],
        ),
        (
            Protocol::Gemini,
            "generationConfig",
            json!({"thinkingConfig":{"vendor":secret},"vendor":secret}),
            vec![
                "generationConfig.vendor",
                "generationConfig.thinkingConfig.vendor",
            ],
        ),
        (
            Protocol::Gemini,
            "toolConfig",
            json!({"functionCallingConfig":{"mode":"AUTO","vendor":secret},"vendor":secret}),
            vec![
                "toolConfig.vendor",
                "toolConfig.functionCallingConfig.vendor",
            ],
        ),
    ];
    for (source, field, value, paths) in cases {
        let mut raw = request(source);
        // 给具名选择提供真实声明，避免测试被“不存在的函数”约束提前挡住。
        raw["tools"] = match source {
            Protocol::OpenAiChat => json!([{"type":"function","function":{"name":"lookup"}}]),
            Protocol::OpenAiResponses => json!([{"type":"function","name":"lookup"}]),
            Protocol::AnthropicMessages => {
                json!([{"name":"lookup","input_schema":{"type":"object"}}])
            }
            Protocol::Gemini => json!([{"functionDeclarations":[{"name":"lookup"}]}]),
        };
        raw[field] = value;
        let ir = source.decode_request(&raw).unwrap();
        assert_eq!(source.encode_request(&ir).unwrap(), raw);
        for path in &paths {
            assert!(
                ir.diagnostics.iter().any(|d| d.path == *path),
                "{source:?}: {path}"
            );
        }
        assert!(
            !serde_json::to_string(&ir.diagnostics)
                .unwrap()
                .contains(secret)
        );
        for target in ALL.into_iter().filter(|target| *target != source) {
            let converted = target.encode_request_for(&ir, &TARGET).unwrap();
            for path in &paths {
                assert!(
                    converted.warnings.iter().any(|w| w.path == *path),
                    "{source:?} -> {target:?}: {path}"
                );
            }
            assert!(
                !serde_json::to_string(&converted.body)
                    .unwrap()
                    .contains(secret)
            );
            assert!(!format!("{:?}", converted.warnings).contains(secret));
        }
    }
}

#[test]
fn explicit_null_extensions_and_private_constraints_keep_their_policy() {
    for source in ALL {
        let mut raw = request(source);
        raw["vendor"] = Value::Null;
        let ir = source.decode_request(&raw).unwrap();
        assert_eq!(source.encode_request(&ir).unwrap(), raw);
        assert!(ir.diagnostics.iter().any(|d| d.path == "request.vendor"));
    }
    for (source, field, value, path) in [
        (
            Protocol::OpenAiResponses,
            "reasoning",
            json!({"context":"private-context"}),
            "reasoning.context",
        ),
        (
            Protocol::AnthropicMessages,
            "container",
            json!({"id":"private-container"}),
            "request.container",
        ),
        (
            Protocol::Gemini,
            "cachedContent",
            json!("cachedContents/private"),
            "request.cachedContent",
        ),
    ] {
        let mut raw = request(source);
        raw[field] = value;
        let ir = source.decode_request(&raw).unwrap();
        assert_eq!(source.encode_request(&ir).unwrap(), raw);
        if source != Protocol::Gemini {
            assert!(
                ir.diagnostics.iter().any(|d| d.reject && d.path == path),
                "{source:?}: {path}"
            );
        } else {
            assert!(ir.cache.reference.is_some());
        }
        for target in ALL.into_iter().filter(|target| *target != source) {
            assert!(target.encode_request_for(&ir, &TARGET).is_err());
        }
    }
}

#[test]
fn declared_unmapped_envelope_fields_have_a_policy_even_when_null() {
    for (source, fields) in [
        (
            Protocol::OpenAiChat,
            vec![
                "audio",
                "function_call",
                "functions",
                "logit_bias",
                "metadata",
                "modalities",
                "moderation",
                "prediction",
                "safety_identifier",
                "service_tier",
                "store",
                "stream_options",
                "user",
                "verbosity",
            ],
        ),
        (
            Protocol::OpenAiResponses,
            vec![
                "access_programs",
                "context_management",
                "include",
                "max_tool_calls",
                "metadata",
                "moderation",
                "safety_identifier",
                "service_tier",
                "store",
                "stream_options",
                "top_logprobs",
                "truncation",
                "user",
            ],
        ),
        (
            Protocol::AnthropicMessages,
            vec!["diagnostics", "inference_geo", "metadata", "service_tier"],
        ),
        (
            Protocol::Gemini,
            vec!["safetySettings", "labels", "serviceTier", "store"],
        ),
    ] {
        let mut raw = request(source);
        for field in &fields {
            raw[*field] = Value::Null;
        }
        let ir = source.decode_request(&raw).unwrap();
        assert_eq!(source.encode_request(&ir).unwrap(), raw);
        for field in &fields {
            assert!(
                ir.diagnostics
                    .iter()
                    .any(|d| d.path == format!("request.{field}")),
                "{source:?}: {field}"
            );
        }
        for target in ALL.into_iter().filter(|target| *target != source) {
            let converted = target.encode_request_for(&ir, &TARGET).unwrap();
            for field in &fields {
                assert!(
                    converted
                        .warnings
                        .iter()
                        .any(|w| w.path == format!("request.{field}")),
                    "{source:?} -> {target:?}: {field}"
                );
            }
        }
    }
    for (source, fields) in [
        (
            Protocol::OpenAiChat,
            vec![
                "metadata",
                "moderation",
                "service_tier",
                "system_fingerprint",
            ],
        ),
        (
            Protocol::OpenAiResponses,
            vec![
                "instructions",
                "metadata",
                "parallel_tool_calls",
                "temperature",
                "tool_choice",
                "tools",
                "top_p",
                "background",
                "completed_at",
                "conversation",
                "max_output_tokens",
                "max_tool_calls",
                "moderation",
                "previous_response_id",
                "prompt",
                "prompt_cache_diagnostics",
                "prompt_cache_key",
                "prompt_cache_options",
                "prompt_cache_retention",
                "reasoning",
                "service_tier",
                "text",
                "truncation",
            ],
        ),
        (
            Protocol::AnthropicMessages,
            vec!["container", "diagnostics", "stop_details", "stop_sequence"],
        ),
    ] {
        let mut raw = response(source);
        for field in &fields {
            raw[*field] = Value::Null;
        }
        let ir = source.decode_response(&raw).unwrap();
        assert_eq!(source.encode_response(&ir).unwrap(), raw);
        for target in ALL.into_iter().filter(|target| *target != source) {
            let converted = target
                .encode_response_for(
                    &ir,
                    &ResponseTarget {
                        model: "target",
                        id: "target",
                        created: 1,
                    },
                )
                .unwrap();
            for field in &fields {
                assert!(
                    converted
                        .warnings
                        .iter()
                        .any(|w| w.path == format!("response.{field}")),
                    "{source:?} -> {target:?}: {field}"
                );
            }
        }
    }
}

#[test]
fn gemini_adaptive_effort_does_not_send_a_conflicting_dynamic_budget() {
    let mut raw = request(Protocol::AnthropicMessages);
    raw["thinking"] = json!({"type":"adaptive"});
    raw["output_config"] = json!({"effort":"low"});
    let mut ir = Protocol::AnthropicMessages.decode_request(&raw).unwrap();
    let converted = Protocol::Gemini.encode_request_for(&ir, &TARGET).unwrap();
    assert_eq!(
        converted.body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "low"
    );
    assert!(
        converted.body["generationConfig"]["thinkingConfig"]
            .get("thinkingBudget")
            .is_none()
    );
    ir.generation.reasoning.budget = Some(1024);
    assert!(Protocol::Gemini.encode_request_for(&ir, &TARGET).is_err());
    ir.generation.reasoning.effort = None;
    let converted = Protocol::Gemini.encode_request_for(&ir, &TARGET).unwrap();
    assert_eq!(
        converted.body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        1024
    );
    assert!(
        converted.body["generationConfig"]["thinkingConfig"]
            .get("thinkingLevel")
            .is_none()
    );
}

#[test]
fn usage_leaf_extensions_are_preserved_or_reported_without_values() {
    for (source, field, leaves) in [
        (
            Protocol::OpenAiChat,
            "usage",
            vec!["prompt_tokens_details", "completion_tokens_details"],
        ),
        (
            Protocol::OpenAiResponses,
            "usage",
            vec!["input_tokens_details", "output_tokens_details"],
        ),
        (
            Protocol::AnthropicMessages,
            "usage",
            vec!["cache_creation", "output_tokens_details"],
        ),
        (
            Protocol::Gemini,
            "usageMetadata",
            vec!["cacheTokensDetails", "toolUsePromptTokensDetails"],
        ),
    ] {
        let mut raw = response(source);
        for leaf in &leaves {
            raw[field][*leaf] = if source == Protocol::Gemini {
                json!([{"modality":"TEXT","tokenCount":0,"vendor":"private-usage"}])
            } else {
                json!({"vendor":"private-usage"})
            };
        }
        let ir = source.decode_response(&raw).unwrap();
        assert_eq!(source.encode_response(&ir).unwrap(), raw);
        assert!(
            !serde_json::to_string(&ir.diagnostics)
                .unwrap()
                .contains("private-usage")
        );
        for target in ALL.into_iter().filter(|target| *target != source) {
            let converted = target
                .encode_response_for(
                    &ir,
                    &ResponseTarget {
                        model: "target",
                        id: "r",
                        created: 1,
                    },
                )
                .unwrap();
            for leaf in &leaves {
                let path = if source == Protocol::Gemini {
                    format!("usage.{leaf}[0].vendor")
                } else {
                    format!("usage.{leaf}.vendor")
                };
                assert!(
                    converted.warnings.iter().any(|w| w.path == path),
                    "{source:?} -> {target:?}: {path}"
                );
            }
            assert!(
                !serde_json::to_string(&converted.body)
                    .unwrap()
                    .contains("private-usage")
            );
        }
    }
}
