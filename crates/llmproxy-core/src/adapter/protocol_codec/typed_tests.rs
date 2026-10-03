//! 直接构造协议类型，验证整个转换链路不需要 JSON 正文。
use super::{ProtocolCodec, RequestTarget, ResponseTarget};
use crate::{
    ir::{
        message::{PartKind, Role},
        request::Item,
    },
    protocol::{self, OptionalNullable as O, Protocol, chat},
};
const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

/// 协议字段直接初始化，未指定参数保持缺失。
fn request() -> protocol::Request {
    use chat::request::message::{Content, ContentMessage, Message};
    protocol::Request::Chat(Box::new(chat::request::Request {
        model: "client-model".into(),
        messages: vec![Message::User(ContentMessage {
            content: Content::Text("hello".into()),
            name: None,
            extra: Default::default(),
        })],
        max_completion_tokens: O::Value(128),
        ..Default::default()
    }))
}

#[test]
fn typed_requests_cross_all_protocols_and_apply_edits() {
    let base = Protocol::OpenAiChat
        .decode_request(&request())
        .unwrap()
        .without_source();
    let target = RequestTarget {
        model: "provider-model",
        max_output_tokens: None,
    };
    for source in ALL {
        let raw = source.encode_request_for(&base, &target).unwrap().body;
        assert_eq!(raw.protocol(), source);
        let mut ir = source.decode_request(&raw).unwrap().without_source();
        ir.generation.temperature = Some(0.5);
        ir.messages[0].parts[0].kind = PartKind::Text("edited".into());
        for target_protocol in ALL {
            let encoded = target_protocol.encode_request_for(&ir, &target).unwrap();
            assert_eq!(encoded.body.protocol(), target_protocol);
            let decoded = target_protocol.decode_request(&encoded.body).unwrap();
            assert_eq!(decoded.generation.temperature, Some(0.5));
            assert_eq!(
                decoded.messages[0].parts[0].kind,
                PartKind::Text("edited".into())
            );
        }
    }
}

#[test]
fn typed_response_candidates_and_usage_cross_all_protocols() {
    use chat::response::{
        completion::{Choice, Completion},
        message::{AssistantRole, Message},
        usage::Usage,
    };
    let raw = protocol::Response::Chat(Box::new(Completion {
        id: "response-1".into(),
        created: 123,
        model: "model".into(),
        object: "chat.completion".into(),
        choices: vec![Choice {
            index: 0,
            finish_reason: "stop".into(),
            logprobs: O::Missing,
            extra: Default::default(),
            message: Message {
                role: AssistantRole::Assistant,
                content: O::Value("answer".into()),
                refusal: O::Null,
                annotations: O::Missing,
                audio: O::Missing,
                function_call: O::Missing,
                tool_calls: O::Missing,
                extra: Default::default(),
            },
        }],
        usage: O::Value(Usage {
            prompt_tokens: 10,
            completion_tokens: 3,
            total_tokens: 13,
            completion_tokens_details: O::Missing,
            prompt_tokens_details: O::Missing,
            extra: Default::default(),
        }),
        ..Default::default()
    }));
    let ir = Protocol::OpenAiChat.decode_response(&raw).unwrap();
    assert_eq!(Protocol::OpenAiChat.encode_response(&ir).unwrap(), raw);
    let target = ResponseTarget {
        model: "client-model",
        id: "response-2",
        created: 456,
    };
    for source in ALL {
        let raw = source
            .encode_response_for(&ir.clone().without_source(), &target)
            .unwrap()
            .body;
        let ir = source.decode_response(&raw).unwrap().without_source();
        for dest in ALL {
            let raw = dest.encode_response_for(&ir, &target).unwrap().body;
            assert_eq!(raw.protocol(), dest);
            let decoded = dest.decode_response(&raw).unwrap();
            assert_eq!(decoded.messages[0].role, Role::Assistant);
            assert_eq!(decoded.usage.unwrap().total_tokens, Some(13));
        }
    }
}

#[test]
fn codec_rejects_mismatched_protocol_carrier() {
    assert!(Protocol::Gemini.decode_request(&request()).is_err());
}

#[test]
fn responses_function_items_are_typed_and_pair_by_call_id() {
    use protocol::responses::{
        function as f,
        request::{
            Request,
            body::{Input, InputItem},
        },
    };
    let raw = protocol::Request::Responses(Box::new(Request {
        model: O::Value("m".into()),
        max_output_tokens: O::Value(64),
        input: O::Value(Input::Items(vec![
            InputItem::FunctionCall(f::Call {
                r#type: f::CallType::FunctionCall,
                id: O::Value("item-1".into()),
                call_id: O::Value("call-1".into()),
                name: "lookup".into(),
                arguments: "{}".into(),
                status: O::Value("completed".into()),
                extra: Default::default(),
            }),
            InputItem::FunctionCallOutput(f::CallOutput {
                r#type: f::ResultType::FunctionCallOutput,
                call_id: O::Value("call-1".into()),
                id: O::Missing,
                output: serde_json::Value::String("done".into()),
                status: O::Missing,
                extra: Default::default(),
            }),
        ])),
        ..Default::default()
    }));
    let ir = Protocol::OpenAiResponses.decode_request(&raw).unwrap();
    assert!(matches!(&ir.items[0],Item::ToolCall{call,..} if call.id.as_deref()==Some("call-1")));
    assert_eq!(Protocol::OpenAiResponses.encode_request(&ir).unwrap(), raw);
    let converted = Protocol::OpenAiChat
        .encode_request_for(
            &ir,
            &RequestTarget {
                model: "m",
                max_output_tokens: None,
            },
        )
        .unwrap();
    let protocol::Request::Chat(body) = converted.body else {
        panic!("目标必须是 Chat 类型")
    };
    assert!(
        matches!(&body.messages[1],chat::request::message::Message::Tool{tool_call_id,..} if tool_call_id=="call-1")
    );
}
