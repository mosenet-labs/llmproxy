use serde_json::{Value, json};

use super::*;
use crate::{
    ir::request::PartKind,
    protocol::{chat, gemini, messages, responses},
};

#[test]
fn chat_round_trip_and_ir_edit() {
    let source = json!([
        {"role":"user","content":"hello","extension":1},
        {"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}","extension":2}}],"name":"agent"},
        {"role":"tool","tool_call_id":"c1","content":"done"}
    ]);
    let raw: Vec<chat::request::message::Message> = serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_chat(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_chat(&ir).unwrap()).unwrap(),
        source
    );
    if let PartKind::Text(text) = &mut ir[0].parts[0].kind {
        *text = "changed".into();
    }
    let encoded = serde_json::to_value(encode_chat(&ir).unwrap()).unwrap();
    assert_eq!(encoded[0]["content"], "changed");
    assert_eq!(encoded[0]["extension"], 1);
}

#[test]
fn messages_round_trip_and_ir_edit() {
    let source = json!([
        {"role":"user","content":"hello","extension":1},
        {"role":"assistant","content":[{"type":"text","text":"checking","cache_control":{"type":"ephemeral"}},{"type":"tool_use","id":"c1","name":"lookup","input":{"q":1},"caller":{"type":"direct"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"done","is_error":false},{"type":"search_result","title":"x"}]}
    ]);
    let raw: Vec<messages::request::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_messages(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_messages(&ir).unwrap()).unwrap(),
        source
    );
    if let PartKind::Text(text) = &mut ir[1].parts[0].kind {
        *text = "changed".into();
    }
    let encoded = serde_json::to_value(encode_messages(&ir).unwrap()).unwrap();
    assert_eq!(encoded[1]["content"][0]["text"], "changed");
    assert_eq!(
        encoded[1]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

#[test]
fn responses_round_trip_and_ir_edit() {
    let source = json!([
        {"role":"user","content":"hello","extension":1},
        {"role":"user","content":[{"type":"input_text","text":"look","extension":2},{"type":"input_image","image_url":"https://example.com/a.png"}],"status":"completed"},
        {"id":"msg_1","content":[{"type":"output_text","text":"answer","annotations":[{"type":"url_citation","url":"https://example.com"}],"extension":3}],"role":"assistant","status":"completed","type":"message","phase":null}
    ]);
    let raw: Vec<responses::request::message::Message> =
        serde_json::from_value(source.clone()).unwrap();
    let mut ir = decode_responses(&raw).unwrap();
    assert_eq!(
        serde_json::to_value(encode_responses(&ir).unwrap()).unwrap(),
        source
    );
    if let PartKind::Text(text) = &mut ir[2].parts[0].kind {
        *text = "edited".into();
    }
    let encoded = serde_json::to_value(encode_responses(&ir).unwrap()).unwrap();
    assert_eq!(encoded[2]["content"][0]["text"], "edited");
    assert_eq!(encoded[2]["content"][0]["extension"], 3);
}

#[test]
fn gemini_round_trip_and_missing_role() {
    let source = json!([
        {"parts":[{"text":"hello","thoughtSignature":"sig","textExtra":1}],"extension":2},
        {"role":"model","parts":[{"functionCall":{"name":"lookup","args":{"q":1},"id":"c1"},"thoughtSignature":"sig"}]},
        {"role":"user","parts":[{"functionResponse":{"name":"lookup","response":{"result":"ok"},"id":"c1"}},{"text":null}]}
    ]);
    let raw: Vec<gemini::request::message::Message> =
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
    assert!(encode_chat(&ir).is_err());
}

#[test]
fn cross_protocol_text_and_tools() {
    let source = json!([{"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"lookup","input":{"q":1}}]}, {"role":"user","content":[{"type":"text","text":"note"},{"type":"tool_result","tool_use_id":"c1","content":"done"}]}]);
    let raw: Vec<messages::request::message::Message> = serde_json::from_value(source).unwrap();
    let ir = decode_messages(&raw).unwrap();
    let chat = encode_chat(&ir).unwrap();
    let encoded = serde_json::to_value(chat).unwrap();
    assert_eq!(encoded[0]["tool_calls"][0]["id"], "c1");
    assert_eq!(encoded[1]["role"], "user");
    assert_eq!(encoded[2]["role"], "tool");
}

#[test]
fn invalid_function_json_cannot_cross_protocol() {
    let raw: Vec<chat::request::message::Message> = serde_json::from_value(json!([{"role":"assistant","tool_calls":[{"type":"function","id":"c1","function":{"name":"lookup","arguments":"not-json"}}]}])).unwrap();
    let ir = decode_chat(&raw).unwrap();
    assert!(encode_messages(&ir).is_err());
    assert!(encode_gemini(&ir).is_err());
    assert_eq!(
        serde_json::to_value(encode_chat(&ir).unwrap()).unwrap()[0]["tool_calls"][0]["function"]["arguments"],
        Value::String("not-json".into())
    );
}

#[test]
fn chat_tool_sequence_merges_for_messages_and_gemini() {
    let source = json!([
        {"role":"assistant","tool_calls":[{"type":"function","id":"c1","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},
        {"role":"user","content":"note"},
        {"role":"tool","tool_call_id":"c1","content":"done"}
    ]);
    let raw: Vec<chat::request::message::Message> = serde_json::from_value(source).unwrap();
    let ir = decode_chat(&raw).unwrap();
    let messages = serde_json::to_value(encode_messages(&ir).unwrap()).unwrap();
    assert_eq!(messages.as_array().unwrap().len(), 2);
    assert_eq!(messages[1]["content"][1]["tool_use_id"], "c1");
    let gemini = serde_json::to_value(encode_gemini(&ir).unwrap()).unwrap();
    assert_eq!(gemini.as_array().unwrap().len(), 2);
    assert_eq!(gemini[1]["parts"][1]["functionResponse"]["name"], "lookup");
    assert_eq!(
        gemini[1]["parts"][1]["functionResponse"]["response"],
        json!({"result":"done"})
    );
}

#[test]
fn gemini_optional_shapes_and_messages_null_content_survive() {
    let gemini_source = json!([{"role":"model","parts":[
        {"functionCall":{"name":"f"}},
        {"functionCall":{"name":"g","id":null,"args":null}}
    ]}]);
    let raw: Vec<gemini::request::message::Message> =
        serde_json::from_value(gemini_source.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(encode_gemini(&decode_gemini(&raw).unwrap()).unwrap()).unwrap(),
        gemini_source
    );

    let messages_source = json!([{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":null}]}]);
    let raw: Vec<messages::request::message::Message> =
        serde_json::from_value(messages_source.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(encode_messages(&decode_messages(&raw).unwrap()).unwrap()).unwrap(),
        messages_source
    );
}
