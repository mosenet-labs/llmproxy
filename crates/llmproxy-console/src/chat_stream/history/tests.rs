//! 不依赖页面组件检查跨来源历史、候选及工具配对。
use super::*;
use crate::chat_stream::request::request_body;
use llmproxy_core::{
    adapter::protocol_codec::ProtocolCodec,
    ir::message::{Part, PartKind, ToolCall, ToolResult},
};
use serde_json::json;

fn selection(protocol: Protocol) -> Selection {
    Selection {
        model_id: protocol.as_str().into(),
        protocol,
    }
}
const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

#[test]
fn mixed_protocol_history_encodes_for_every_target_and_mode() {
    let mut history = Conversation::default();
    for (index, protocol) in ALL.into_iter().enumerate() {
        history.append(
            &selection(protocol),
            Content::text(Role::User, &format!("question-{index}")),
        );
        history.append(
            &selection(protocol),
            Content::text(Role::Assistant, &format!("answer-{index}")),
        );
    }
    for target in ALL {
        for stream in [false, true] {
            let result = request_body(&selection(target), "model", &history, stream).unwrap();
            assert!(result.warnings.is_empty());
            let decoded = target.decode_request(&result.body).unwrap();
            assert_eq!(decoded.messages.len(), 8);
            for (index, message) in decoded.messages.iter().enumerate() {
                let text = if index % 2 == 0 {
                    format!("question-{}", index / 2)
                } else {
                    format!("answer-{}", index / 2)
                };
                assert_eq!(message.parts[0].kind, PartKind::Text(text));
            }
        }
    }
}

fn tool_history(result: bool) -> Conversation {
    let mut history = Conversation::default();
    let origin = selection(Protocol::OpenAiChat);
    history.append(&origin, Content::text(Role::User, "check"));
    let mut content = Content {
        messages: vec![Message {
            role: Role::Assistant,
            parts: vec![Part {
                kind: PartKind::ToolCall(ToolCall {
                    id: Some("call-1".into()),
                    name: "lookup".into(),
                    arguments: json!({"private":"value"}),
                }),
                metadata: Default::default(),
            }],
            metadata: Default::default(),
        }],
        items: vec![Item::Message(0)],
    };
    if result {
        content.items.push(Item::ToolResult(ToolResult {
            id: Some("call-1".into()),
            name: Some("lookup".into()),
            content: "found".into(),
        }));
    }
    history.append(&origin, content);
    history
}

#[test]
fn paired_tools_survive_and_unanswered_tools_are_skipped_without_erasing_history() {
    for target in ALL {
        let paired = request_body(&selection(target), "model", &tool_history(true), false).unwrap();
        let decoded = target.decode_request(&paired.body).unwrap();
        assert!(
            decoded
                .messages
                .iter()
                .flat_map(|message| &message.parts)
                .any(|part| matches!(part.kind, PartKind::ToolCall(_)))
                || decoded
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::ToolCall { .. }))
        );
        let history = tool_history(false);
        let result = request_body(&selection(target), "model", &history, false).unwrap();
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.reason.contains("尚未取得结果"))
        );
        assert!(!format!("{:?}", result.warnings).contains("private"));
        assert!(format!("{history:?}").contains("private"));
        let decoded = target.decode_request(&result.body).unwrap();
        assert_eq!(decoded.messages.len(), 1);
    }
}

/// HTTP 夹具只在测试入口解码，生产历史处理不经过整包 JSON。
fn parsed(protocol: Protocol, value: serde_json::Value) -> Content {
    let raw = llmproxy_core::protocol::wire::decode_request(
        protocol,
        &serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let ir = protocol.decode_request(&raw).unwrap();
    Content {
        messages: ir.messages,
        items: ir.items,
    }
}

#[test]
fn structured_tool_result_arrays_and_error_flags_use_core_mapping() {
    let source = selection(Protocol::AnthropicMessages);
    let mut history = Conversation::default();
    history.append(&source, parsed(source.protocol, json!({
        "model":"m","max_tokens":1,"messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"c","name":"lookup","input":{"q":1}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"c","is_error":true,"content":[{"type":"text","text":"failed lookup"}]}]}
        ]
    })));
    for target in ALL {
        let result = request_body(&selection(target), "m", &history, false).unwrap();
        let text =
            String::from_utf8(llmproxy_core::protocol::wire::encode_request(&result.body).unwrap())
                .unwrap();
        assert!(text.contains("failed lookup"));
        if target == Protocol::AnthropicMessages {
            assert!(text.contains("\"is_error\":true"));
        } else if target == Protocol::Gemini {
            assert!(text.contains("\"error\""));
        } else {
            assert!(!result.warnings.is_empty());
        }
    }
}

#[test]
fn unsigned_tools_are_portable_but_signatures_and_continuations_are_scoped() {
    for signed in [false, true] {
        let source = selection(Protocol::Gemini);
        let mut history = Conversation::default();
        let mut call = json!({"functionCall":{"id":"c","name":"lookup","args":{}}});
        if signed {
            call["thoughtSignature"] = json!("private-signature");
        }
        history.append(&source, parsed(source.protocol, json!({"contents":[
            {"role":"model","parts":[call]},
            {"role":"user","parts":[{"functionResponse":{"id":"c","name":"lookup","response":{"value":1}}}]}
        ]})));
        let same = request_body(&source, "m", &history, false).unwrap();
        assert!(
            String::from_utf8(llmproxy_core::protocol::wire::encode_request(&same.body).unwrap())
                .unwrap()
                .contains("lookup")
        );
        if signed {
            assert!(format!("{:?}", same.body).contains("private-signature"));
        }
        for mut target in ALL.map(selection) {
            target.model_id = "different-model".into();
            let prepared = history.request(&target);
            assert_eq!(prepared.body.messages.is_empty(), signed);
            assert_eq!(prepared.warnings.is_empty(), !signed);
            assert!(!format!("{:?}", prepared.warnings).contains("private-signature"));
        }
    }
    let mut history = tool_history(true);
    for message in &mut history.records[1].content.messages {
        for part in &mut message.parts {
            if let PartKind::ToolCall(call) = &mut part.kind {
                call.id = Some(format!(
                    "{}ref",
                    llmproxy_core::ir::message::TOOL_CONTINUATION_ID_PREFIX
                ));
            }
        }
    }
    if let Item::ToolResult(result) = &mut history.records[1].content.items[1] {
        result.id = Some(format!(
            "{}ref",
            llmproxy_core::ir::message::TOOL_CONTINUATION_ID_PREFIX
        ));
    }
    assert_eq!(
        history
            .request(&selection(Protocol::Gemini))
            .body
            .messages
            .len(),
        1
    );
    assert!(
        history
            .request(&selection(Protocol::OpenAiChat))
            .warnings
            .is_empty()
    );
}

#[test]
fn duplicate_calls_and_orphan_results_never_form_tool_context() {
    let mut history = tool_history(true);
    let duplicate = history.records[1].clone();
    history.records.push(duplicate);
    let prepared = history.request(&selection(Protocol::OpenAiChat));
    assert_eq!(prepared.body.messages.len(), 1);
    assert!(prepared.warnings.iter().any(|w| w.reason.contains("重复")));
    assert!(
        prepared
            .warnings
            .iter()
            .any(|w| w.reason.contains("前置调用"))
    );
}

#[test]
fn private_media_and_extensions_are_retained_in_storage_and_warn_without_values() {
    let source = selection(Protocol::OpenAiResponses);
    let mut history = Conversation::default();
    history.append(&source, parsed(source.protocol,json!({"model":"m","input":[{"role":"user","content":[{"type":"input_file","file_id":"private-file"},{"type":"input_text","text":"keep","private-extension":"secret"}]}]})));
    assert!(format!("{:?}", history).contains("private-file"));
    let same = history.request(&source);
    assert!(format!("{:?}", same.body).contains("private-file"));
    let cross = history.request(&selection(Protocol::Gemini));
    assert_eq!(cross.body.messages[0].parts.len(), 1);
    assert_eq!(
        cross.body.messages[0].parts[0].kind,
        PartKind::Text("keep".into())
    );
    assert!(cross.warnings.len() >= 2);
    assert!(!format!("{:?}", cross.warnings).contains("secret"));
    assert!(!format!("{:?}", cross.warnings).contains("private-file"));
}
