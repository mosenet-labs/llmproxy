use serde_json::{Value, json};

use super::*;
use crate::{
    ir::response::PartKind,
    protocol::{chat, gemini, messages, responses},
};

#[test]
fn chat_response_round_trip_and_ir_edit() {
    let source = json!([{"role":"assistant","content":"hello","refusal":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}","extension":1}}],"extension":2}]);
    let raw: Vec<chat::response::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_chat(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_chat(&ir).unwrap()).unwrap(),
        source
    );
    if let PartKind::Text(text) = &mut ir[0].parts[0].kind {
        *text = "edited".into();
    }
    let encoded = serde_json::to_value(encode_chat(&ir).unwrap()).unwrap();
    assert_eq!(encoded[0]["content"], "edited");
    assert_eq!(encoded[0]["tool_calls"][0]["function"]["extension"], 1);
}

#[test]
fn responses_output_message_round_trip_and_ir_edit() {
    let source = json!([{"id":"msg_1","content":[{"type":"output_text","text":"hello","annotations":[{"type":"file_citation","file_id":"f1"}],"extension":1},{"type":"refusal","refusal":"no"}],"role":"assistant","status":"completed","type":"message","phase":null,"extension":2}]);
    let raw: Vec<responses::response::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_responses(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_responses(&ir).unwrap()).unwrap(),
        source
    );
    assert!(encode_chat(&ir).is_err());
    if let PartKind::Text(text) = &mut ir[0].parts[0].kind {
        *text = "edited".into();
    }
    let encoded = serde_json::to_value(encode_responses(&ir).unwrap()).unwrap();
    assert_eq!(encoded[0]["content"][0]["text"], "edited");
    assert_eq!(encoded[0]["content"][0]["annotations"][0]["file_id"], "f1");
}

#[test]
fn messages_response_round_trip_and_ir_edit() {
    let source = json!([{"id":"msg_1","content":[{"type":"text","text":"hello","citations":null},{"type":"thinking","signature":"sig","thinking":"plan"},{"type":"tool_use","id":"c1","name":"lookup","input":{"q":1},"caller":{"type":"direct"}},{"type":"server_tool_use","id":"srv_1","name":"search","input":{}}],"model":"claude-example","role":"assistant","stop_reason":"tool_use","stop_sequence":null,"type":"message","usage":{"input_tokens":3,"output_tokens":4},"extension":2}]);
    let raw: Vec<messages::response::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_messages(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_messages(&ir).unwrap()).unwrap(),
        source
    );
    if let PartKind::Text(text) = &mut ir[0].parts[0].kind {
        *text = "edited".into();
    }
    let encoded = serde_json::to_value(encode_messages(&ir).unwrap()).unwrap();
    assert_eq!(encoded[0]["content"][0]["text"], "edited");
    assert_eq!(encoded[0]["content"][1]["signature"], "sig");
}

#[test]
fn gemini_response_round_trip_and_missing_role() {
    let source = json!([{"parts":[{"text":"hello","thoughtSignature":"sig"},{"functionCall":{"name":"lookup","id":"c1","args":{"q":1}}}],"extension":2}]);
    let raw: Vec<gemini::response::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_gemini(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_gemini(&ir).unwrap()).unwrap(),
        source
    );
    if let PartKind::Text(text) = &mut ir[0].parts[0].kind {
        *text = "edited".into();
    }
    let encoded = serde_json::to_value(encode_gemini(&ir).unwrap()).unwrap();
    assert_eq!(encoded[0]["parts"][0]["text"], "edited");
    assert!(encoded[0].get("role").is_none());
}

#[test]
fn cross_protocol_text_and_function_call() {
    let source = json!([{"role":"assistant","content":"hello","tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}"}}]}]);
    let raw: Vec<chat::response::message::Message> = serde_json::from_value(source).unwrap();
    let ir = decode_chat(&raw).unwrap();
    let gemini = serde_json::to_value(encode_gemini(&ir).unwrap()).unwrap();
    assert_eq!(gemini[0]["parts"][0]["text"], "hello");
    assert_eq!(gemini[0]["parts"][1]["functionCall"]["name"], "lookup");

    let source = json!([{"id":"msg_1","content":[{"type":"text","text":"answer"}],"model":"claude-example","role":"assistant","stop_reason":"end_turn","stop_sequence":null,"type":"message","usage":{"input_tokens":1,"output_tokens":1}}]);
    let raw: Vec<messages::response::message::Message> = serde_json::from_value(source).unwrap();
    let ir = decode_messages(&raw).unwrap();
    let chat = serde_json::to_value(encode_chat(&ir).unwrap()).unwrap();
    assert_eq!(chat[0]["content"], "answer");

    let source = json!([{"id":"msg_2","content":[{"type":"tool_use","id":"c2","name":"lookup","input":{"q":2},"caller":{"type":"direct"}}],"model":"claude-example","role":"assistant","stop_reason":"tool_use","stop_sequence":null,"type":"message","usage":{"input_tokens":1,"output_tokens":1}}]);
    let raw: Vec<messages::response::message::Message> = serde_json::from_value(source).unwrap();
    let ir = decode_messages(&raw).unwrap();
    let chat = serde_json::to_value(encode_chat(&ir).unwrap()).unwrap();
    assert_eq!(chat[0]["tool_calls"][0]["function"]["name"], "lookup");
}

#[test]
fn missing_output_context_and_opaque_cross_protocol_are_rejected() {
    let raw: Vec<chat::response::message::Message> =
        serde_json::from_value(json!([{"role":"assistant","content":"hello"}])).unwrap();
    let ir = decode_chat(&raw).unwrap();
    assert!(encode_responses(&ir).is_err());
    assert!(encode_messages(&ir).is_err());

    let raw: Vec<messages::response::message::Message> = serde_json::from_value(json!([{"id":"msg_1","content":[{"type":"thinking","signature":"sig","thinking":"plan"}],"model":"claude-example","role":"assistant","stop_reason":null,"stop_sequence":null,"type":"message","usage":{"input_tokens":1,"output_tokens":1}}])).unwrap();
    let ir = decode_messages(&raw).unwrap();
    assert!(encode_chat(&ir).is_err());
    assert!(matches!(&ir[0].parts[0].kind, PartKind::Opaque(_)));
    assert_eq!(
        serde_json::to_value(encode_messages(&ir).unwrap()).unwrap()[0]["content"][0]["signature"],
        Value::String("sig".into())
    );
}

#[test]
fn signatures_citations_and_server_tool_caller_do_not_silently_disappear() {
    let raw: Vec<gemini::response::message::Message> = serde_json::from_value(
        json!([{"role":"model","parts":[{"text":"answer","thoughtSignature":"sig"}]}]),
    )
    .unwrap();
    assert!(encode_chat(&decode_gemini(&raw).unwrap()).is_err());

    let raw: Vec<messages::response::message::Message> = serde_json::from_value(json!([{"id":"msg_1","content":[{"type":"text","text":"answer","citations":[{"type":"char_location","cited_text":"answer"}]},{"type":"tool_use","id":"c1","name":"lookup","input":{},"caller":{"type":"server_tool","tool_id":"srv"}}],"model":"claude-example","role":"assistant","stop_reason":"tool_use","stop_sequence":null,"type":"message","usage":{"input_tokens":1,"output_tokens":1}}])).unwrap();
    let ir = decode_messages(&raw).unwrap();
    assert!(encode_chat(&ir).is_err());
    assert!(matches!(ir[0].parts[1].kind, PartKind::Opaque(_)));
    assert_eq!(
        serde_json::to_value(encode_messages(&ir).unwrap()).unwrap()[0]["content"][1]["caller"]["tool_id"],
        "srv"
    );
}

#[test]
fn chat_audio_and_optional_fields_round_trip_without_cross_protocol_loss() {
    let source = json!([{"role":"assistant","content":null,"annotations":[],"audio":{"id":"audio_1","data":"YWJj","expires_at":1700000000,"transcript":"hello"},"tool_calls":null}]);
    let raw: Vec<chat::response::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let ir = decode_chat(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_chat(&ir).unwrap()).unwrap(),
        source
    );
    assert!(encode_gemini(&ir).is_err());
}
