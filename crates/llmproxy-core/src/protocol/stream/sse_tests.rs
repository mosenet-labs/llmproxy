use super::sse::{self, Decoder};
use crate::protocol::Protocol;

#[test]
fn framing_handles_every_byte_split_bom_line_endings_and_multiline_data() {
    for separator in ["\n", "\r\n", "\r"] {
        let raw = format!(
            "\u{feff}: 注释{separator}event: custom{separator}id: ignored{separator}data: {{\"text\":{separator}data: \"你好\"}}{separator}{separator}"
        );
        for split in 0..=raw.len() {
            let mut decoder = Decoder::new(1024);
            let mut frames = Vec::new();
            for chunk in [&raw.as_bytes()[..split], &raw.as_bytes()[split..]] {
                for byte in chunk {
                    if let Some(frame) = decoder.push(*byte).unwrap() {
                        frames.push(frame);
                    }
                }
            }
            decoder.finish().unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].event.as_deref(), Some("custom"));
            assert_eq!(frames[0].data, b"{\"text\":\n\"\xe4\xbd\xa0\xe5\xa5\xbd\"}");
        }
    }
}

#[test]
fn framing_bounds_all_fields_and_rejects_invalid_utf8_and_truncation() {
    let mut decoder = Decoder::new(16);
    assert!((0..17).any(|_| decoder.push(b':').is_err()));
    let mut decoder = Decoder::new(1024);
    for byte in b"data: {}\n" {
        decoder.push(*byte).unwrap();
    }
    assert!(decoder.finish().is_err());
    let mut decoder = Decoder::new(1024);
    for byte in b"data: \xff" {
        decoder.push(*byte).unwrap();
    }
    assert!(decoder.push(b'\n').is_err());
}

#[test]
fn typed_sse_events_validate_names_and_bound_serialized_output() {
    let mut decoder = Decoder::new(1024);
    let mut frame = None;
    for byte in b"event: not_ping\ndata: {\"type\":\"ping\"}\n\n" {
        if let Some(next) = decoder.push(*byte).unwrap() {
            frame = Some(next);
        }
    }
    assert!(sse::decode(Protocol::AnthropicMessages, &frame.unwrap()).is_err());
    let frame = sse::Frame {
        event: None,
        data: b"[DONE]".to_vec(),
    };
    assert!(sse::decode(Protocol::Gemini, &frame).is_err());
    let raw = sse::decode(Protocol::OpenAiChat, &frame).unwrap();
    assert_eq!(sse::encode(&raw, 64).unwrap(), b"data: [DONE]\n\n");
    assert!(sse::encode(&raw, 8).is_err());
    let frame = sse::Frame {
        event: Some("ping".into()),
        data: b"{\"type\":\"ping\"}".to_vec(),
    };
    let raw = sse::decode(Protocol::AnthropicMessages, &frame).unwrap();
    assert!(sse::encode(&raw, 8).is_err());
    assert!(
        sse::encode(&raw, 128)
            .unwrap()
            .starts_with(b"event: ping\n")
    );
}

#[test]
fn chat_and_gemini_error_envelopes_cannot_be_treated_as_normal_chunks() {
    for (protocol, value) in [
        (
            Protocol::Gemini,
            serde_json::json!({"error":{"code":429,"message":"private"}}),
        ),
        (
            Protocol::OpenAiChat,
            serde_json::json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[],"error":{"message":"private"}}),
        ),
    ] {
        let mut frame = sse::Frame {
            event: None,
            data: serde_json::to_vec(&value).unwrap(),
        };
        assert!(sse::decode(protocol, &frame).is_err());
        let mut normal = value;
        normal["error"] = serde_json::Value::Null;
        frame.data = serde_json::to_vec(&normal).unwrap();
        assert!(sse::decode(protocol, &frame).is_ok());
    }
}
