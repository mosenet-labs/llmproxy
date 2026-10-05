//! 父请求只解析一次，子请求按原输入边界读取并只交付一次目标正文。
use super::{MAX_BUFFERED_BODY, RequestBody, codec};
use bytes::Bytes;
use llmproxy_core::{
    ir::message::PartKind,
    protocol::{Protocol, Request},
};

#[tokio::test]
async fn prepared_target_body_is_emitted_only_at_end() {
    let mut request = RequestBody::new();
    request.set_cross_protocol(Protocol::OpenAiChat, Protocol::AnthropicMessages, "target");
    let chunks = [
        Bytes::from_static(br#"{"model":"alias","messages":[{"role":"user","content":"hi"}],"#),
        Bytes::from_static(br#""max_completion_tokens":32,"stream":true}"#),
    ];
    let ir = request.decode_cross_request(&chunks, false).unwrap();
    assert!(ir.generation.stream);
    request.prepare_cross_request(&ir).await.unwrap();
    let length = request.prepared_length().unwrap();
    for chunk in chunks {
        let mut body = Some(chunk);
        request.push(&mut body, false).await.unwrap();
        assert!(body.unwrap().is_empty());
        assert_eq!(request.prepared_length(), Some(length));
    }
    let mut body = None;
    request.push(&mut body, true).await.unwrap();
    let body = body.unwrap();
    assert_eq!(body.len(), length);
    let Request::Messages(body) =
        codec::decode_request(Protocol::AnthropicMessages, &body).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(body.model, "target");
    assert_eq!(body.stream.as_option(), Some(&true));
    assert!(request.prepared_length().is_none());
    assert!(request.push(&mut None, true).await.is_err());
}

#[test]
fn gemini_stream_intent_comes_from_url_and_json_protocols_from_body() {
    let mut request = RequestBody::new();
    request.set_cross_protocol(Protocol::Gemini, Protocol::OpenAiChat, "target");
    let raw = Bytes::from_static(
        br#"{"contents":[{"role":"user","parts":[{"text":"hi"}]}],"stream":true}"#,
    );
    for stream in [false, true] {
        let ir = request
            .decode_cross_request(std::slice::from_ref(&raw), stream)
            .unwrap();
        assert_eq!(ir.generation.stream, stream);
    }
    request.set_cross_protocol(Protocol::OpenAiChat, Protocol::Gemini, "target");
    let raw =
        Bytes::from_static(br#"{"model":"alias","messages":[{"role":"user","content":"hi"}]}"#);
    let ir = request.decode_cross_request(&[raw], true).unwrap();
    assert!(!ir.generation.stream, "正文协议不使用外部标记猜测流式意图");
}

#[tokio::test]
async fn prepared_body_and_raw_input_have_independent_size_limits() {
    let mut request = RequestBody::new();
    request.set_cross_protocol(Protocol::OpenAiChat, Protocol::AnthropicMessages, "target");
    let raw = Bytes::from_static(br#"{"model":"alias","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32}"#);
    let mut ir = request.decode_cross_request(&[raw], false).unwrap();
    ir.messages[0].parts[0].kind = PartKind::Text("a".repeat(MAX_BUFFERED_BODY));
    let error = request.prepare_cross_request(&ir).await.unwrap_err();
    assert_eq!(error.etype(), &pingora::ErrorType::HTTPStatus(413));
    assert!(request.prepared_length().is_none());
    let oversized = Bytes::from(vec![b' '; MAX_BUFFERED_BODY + 1]);
    let error = request
        .decode_cross_request(&[oversized], false)
        .err()
        .unwrap();
    assert_eq!(error.etype(), &pingora::ErrorType::HTTPStatus(413));
}
