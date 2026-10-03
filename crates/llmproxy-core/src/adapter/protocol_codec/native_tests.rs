//! 内置工具与服务端输出的可移植语义回归。
use super::{RequestTarget, ResponseTarget, cross_tests::response, json_test_support::JsonCodec};
use crate::protocol::Protocol;
use serde_json::json;
const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];
const TARGET: RequestTarget<'static> = RequestTarget {
    model: "target",
    max_output_tokens: Some(128),
};

#[test]
fn search_declarations_can_be_rebuilt_without_source_in_all_protocols() {
    for (source, raw) in [
        (
            Protocol::OpenAiChat,
            json!({"model":"m","messages":[{"role":"user","content":"search"}],"web_search_options":{}}),
        ),
        (
            Protocol::OpenAiResponses,
            json!({"model":"m","input":"search","tools":[{"type":"web_search"}]}),
        ),
        (
            Protocol::AnthropicMessages,
            json!({"model":"m","max_tokens":128,"messages":[{"role":"user","content":"search"}],"tools":[{"type":"web_search_20250305","name":"web_search"}]}),
        ),
        (
            Protocol::Gemini,
            json!({"contents":[{"role":"user","parts":[{"text":"search"}]}],"tools":[{"googleSearch":{}}]}),
        ),
    ] {
        let ir = source.decode_request(&raw).unwrap();
        assert_eq!(source.encode_request(&ir).unwrap(), raw);
        assert_eq!(ir.native_tools.len(), 1);
        for target in ALL {
            let result = target
                .encode_request_for(&ir.clone().without_source(), &TARGET)
                .unwrap();
            let decoded = target.decode_request(&result.body).unwrap();
            assert_eq!(decoded.native_tools.len(), 1, "{source:?}->{target:?}");
            assert!(decoded.tools.is_empty());
        }
    }
}

#[test]
fn search_filters_and_private_execution_state_are_never_silently_removed() {
    let raw = json!({"model":"m","input":"search","tools":[{"type":"web_search","filters":{"allowed_domains":["example.com"]}}]});
    let ir = Protocol::OpenAiResponses
        .decode_request(&raw)
        .unwrap()
        .without_source();
    let encoded = Protocol::AnthropicMessages
        .encode_request_for(&ir, &TARGET)
        .unwrap();
    assert_eq!(
        encoded.body["tools"][0]["allowed_domains"],
        json!(["example.com"])
    );
    for target in [Protocol::OpenAiChat, Protocol::Gemini] {
        assert!(target.encode_request_for(&ir, &TARGET).is_err());
    }
    for tool in [
        json!({"type":"code_interpreter","container":"private"}),
        json!({"type":"code_interpreter","container":{"type":"auto","file_ids":["private"]}}),
        json!({"type":"file_search","vector_store_ids":["private"]}),
    ] {
        let raw = json!({"model":"m","input":"hi","tools":[tool]});
        let ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
        assert_eq!(Protocol::OpenAiResponses.encode_request(&ir).unwrap(), raw);
        assert!(Protocol::Gemini.encode_request_for(&ir, &TARGET).is_err());
    }
}

#[test]
fn code_execution_maps_to_fresh_target_environment_or_reports_absence() {
    let raw = json!({"contents":[{"role":"user","parts":[{"text":"calculate"}]}],"tools":[{"codeExecution":{}}]});
    let ir = Protocol::Gemini
        .decode_request(&raw)
        .unwrap()
        .without_source();
    for target in ALL {
        let encoded = target.encode_request_for(&ir, &TARGET).unwrap();
        assert_eq!(
            target
                .decode_request(&encoded.body)
                .unwrap()
                .native_tools
                .len(),
            usize::from(target != Protocol::OpenAiChat)
        );
        assert!(
            encoded
                .warnings
                .iter()
                .any(|w| w.path == "native_tools.code_execution")
        );
    }
}

#[test]
fn server_outputs_remain_visible_but_do_not_become_client_calls() {
    let target = ResponseTarget {
        model: "target",
        id: "id",
        created: 1,
    };
    for source in [
        Protocol::Gemini,
        Protocol::AnthropicMessages,
        Protocol::OpenAiResponses,
    ] {
        let mut raw = response(source);
        match source {
            Protocol::Gemini => {
                raw["candidates"][0]["content"]["parts"] = json!([{"executableCode":{"language":"PYTHON","code":"print(42)"}},{"codeExecutionResult":{"outcome":"OUTCOME_OK","output":"42"}}])
            }
            Protocol::AnthropicMessages => {
                raw["content"] = json!([{"type":"server_tool_use","id":"server_id","name":"code_execution","input":{"code":"print(42)"}},{"type":"code_execution_tool_result","tool_use_id":"server_id","content":{"type":"code_execution_result","stdout":"42","stderr":"","return_code":0,"content":[]}}])
            }
            Protocol::OpenAiResponses => {
                raw["output"] = json!([{"type":"code_interpreter_call","id":"server_id","container_id":"private","status":"completed","code":"print(42)","outputs":[{"type":"logs","logs":"42"}]}])
            }
            _ => unreachable!(),
        }
        let ir = source.decode_response(&raw).unwrap();
        assert_eq!(source.encode_response(&ir).unwrap(), raw);
        for protocol in ALL {
            let encoded = protocol
                .encode_response_for(&ir.clone().without_source(), &target)
                .unwrap();
            let decoded = protocol.decode_response(&encoded.body).unwrap();
            assert_eq!(
                decoded.candidates[0].finish_reason,
                crate::ir::response::FinishReason::Stop
            );
            assert!(decoded.messages.iter().flat_map(|m| &m.parts).any(
                |p| matches!(&p.kind,crate::ir::message::PartKind::Text(t) if t.contains("42"))
            ));
            assert!(!encoded.body.to_string().contains("server_id"));
            assert!(!encoded.body.to_string().contains("private"));
            assert!(!encoded.warnings.is_empty());
        }
    }
}

#[test]
fn server_output_request_adapters_keep_same_protocol_records() {
    let raw = json!({"role":"assistant","content":[{"type":"server_tool_use","id":"s","name":"code_execution","input":{}},{"type":"code_execution_tool_result","tool_use_id":"s","content":{"type":"code_execution_result","stdout":"42","stderr":"","return_code":0,"content":[]}}]});
    let messages = vec![serde_json::from_value(raw.clone()).unwrap()];
    let ir = crate::adapter::request::decode_messages(&messages).unwrap();
    let encoded = crate::adapter::request::encode_messages(&ir).unwrap();
    assert_eq!(serde_json::to_value(&encoded[0]).unwrap(), raw);
}

#[test]
fn opaque_resource_inputs_are_rejected_and_unknown_tools_are_reported() {
    for (protocol, raw) in [
        (
            Protocol::OpenAiResponses,
            json!({"model":"m","input":[{"type":"item_reference","id":"private"},{"role":"user","content":"hi"}]}),
        ),
        (
            Protocol::AnthropicMessages,
            json!({"model":"m","max_tokens":128,"messages":[{"role":"user","content":[{"type":"container_upload","file_id":"private"},{"type":"text","text":"hi"}]}]}),
        ),
    ] {
        let ir = protocol.decode_request(&raw).unwrap();
        assert_eq!(protocol.encode_request(&ir).unwrap(), raw);
        assert!(Protocol::Gemini.encode_request_for(&ir, &TARGET).is_err());
    }
    let raw = json!({"model":"m","input":"hi","tools":[{"type":"computer_use_preview","display_width":1024,"display_height":768,"environment":"browser"}]});
    let ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
    let output = Protocol::Gemini.encode_request_for(&ir, &TARGET).unwrap();
    assert!(output.warnings.iter().any(|w| w.path == "tools[0]"));
    assert!(
        Protocol::Gemini
            .decode_request(&output.body)
            .unwrap()
            .native_tools
            .is_empty()
    );
}

#[test]
fn required_and_disabled_native_tools_keep_the_choice_constraint() {
    let raw = json!({"model":"m","input":"search","tools":[{"type":"web_search"}],"tool_choice":"required"});
    let mut ir = Protocol::OpenAiResponses
        .decode_request(&raw)
        .unwrap()
        .without_source();
    assert!(
        Protocol::AnthropicMessages
            .encode_request_for(&ir, &TARGET)
            .is_ok()
    );
    // Gemini 的函数调用模式不能强制 Google Search，不能把 required 降为 auto。
    assert!(Protocol::Gemini.encode_request_for(&ir, &TARGET).is_err());
    ir.generation.tool_choice = Some(crate::ir::request::controls::ToolChoice::None);
    for target in ALL {
        let result = target.encode_request_for(&ir, &TARGET).unwrap();
        assert!(
            target
                .decode_request(&result.body)
                .unwrap()
                .native_tools
                .is_empty()
        );
    }
}
