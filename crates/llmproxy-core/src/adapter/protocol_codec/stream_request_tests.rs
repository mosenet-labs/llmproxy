//! 流式请求仍走类型化协议 → IR → 目标协议；Gemini 的模式属于 HTTP 层。
use super::{ProtocolCodec, RequestTarget};
use crate::protocol::{self, OptionalNullable as O, Protocol, chat};

const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

fn request() -> protocol::Request {
    use chat::request::message::{Content, ContentMessage, Message};
    protocol::Request::Chat(Box::new(chat::request::Request {
        model: "model".into(),
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
fn stream_requests_cross_all_protocols_without_source_body() {
    let base = Protocol::OpenAiChat
        .decode_request(&request())
        .unwrap()
        .without_source();
    let target = RequestTarget {
        model: "model",
        max_output_tokens: None,
    };
    for source in ALL {
        let original = source.encode_request_for(&base, &target).unwrap().body;
        let mut ir = source.decode_request(&original).unwrap().without_source();
        ir.generation.stream = true;
        for dest in ALL {
            let encoded = dest.encode_request_for(&ir, &target).unwrap().body;
            assert_eq!(encoded.protocol(), dest);
            match &encoded {
                protocol::Request::Chat(body) => {
                    assert_eq!(body.stream, O::Value(true));
                    assert_eq!(
                        body.stream_options.as_option().unwrap().include_usage,
                        O::Value(true)
                    );
                }
                protocol::Request::Responses(body) => assert_eq!(body.stream, O::Value(true)),
                protocol::Request::Messages(body) => assert_eq!(body.stream, O::Value(true)),
                protocol::Request::Gemini(body) => {
                    assert!(!body.extra.contains_key("stream"));
                    assert!(!body.extra.contains_key("model"));
                }
            }
            let decoded = dest.decode_request(&encoded).unwrap();
            assert_eq!(decoded.generation.stream, dest != Protocol::Gemini);
            assert_eq!(decoded.generation.max_output_tokens, Some(128));
            assert_eq!(
                decoded.messages[0].parts[0].kind,
                ir.messages[0].parts[0].kind
            );
            assert!(ir.generation.stream, "目标正文编码不能修改 HTTP 流式意图");
        }
    }
}

#[test]
fn same_protocol_stream_edits_keep_native_options() {
    let protocol::Request::Chat(mut body) = request() else {
        unreachable!()
    };
    body.stream = O::Value(true);
    body.stream_options = O::Value(chat::request::body::StreamOptions {
        include_usage: O::Value(false),
        include_obfuscation: O::Value(false),
        extra: Default::default(),
    });
    let original = protocol::Request::Chat(body);
    let mut ir = Protocol::OpenAiChat.decode_request(&original).unwrap();
    assert_eq!(Protocol::OpenAiChat.encode_request(&ir).unwrap(), original);
    ir.generation.stream = false;
    let protocol::Request::Chat(body) = Protocol::OpenAiChat.encode_request(&ir).unwrap() else {
        unreachable!()
    };
    assert_eq!(body.stream, O::Value(false));
    let protocol::Request::Chat(original) = original else {
        unreachable!()
    };
    assert_eq!(body.stream_options, original.stream_options);

    let base = Protocol::OpenAiChat
        .decode_request(&request())
        .unwrap()
        .without_source();
    for protocol in ALL {
        let original = protocol.encode_request(&base).unwrap();
        let mut ir = protocol.decode_request(&original).unwrap();
        ir.generation.stream = true;
        let stream = protocol.encode_request(&ir).unwrap();
        if protocol == Protocol::Gemini {
            assert_eq!(stream, original, "Gemini 只修改 HTTP 方法，正文保持原样");
        } else {
            assert!(protocol.decode_request(&stream).unwrap().generation.stream);
        }
        ir.generation.stream = false;
        assert_eq!(protocol.encode_request(&ir).unwrap(), original);
    }
}
