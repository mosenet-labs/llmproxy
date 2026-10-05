//! 上游 HTTP 头独立于正文编解码，四协议和两种模式均按目标生成。
use super::{RequestContext, request};
use crate::{
    observability::RequestTelemetry,
    snapshot::ResolvedProvider,
    transform::{BodyTransform, RequestBody},
};
use bytes::Bytes;
use llmproxy_core::protocol::{MessagesAuth, Protocol};
use pingora_http::RequestHeader;
use std::sync::Arc;

const ALL: [Protocol; 4] = [
    Protocol::OpenAiChat,
    Protocol::OpenAiResponses,
    Protocol::AnthropicMessages,
    Protocol::Gemini,
];

fn context(source: Protocol, target: Protocol, stream: bool) -> RequestContext {
    RequestContext {
        telemetry: RequestTelemetry::new(),
        protocol: Some(source),
        client_model: Some("alias".into()),
        response_id: "response".into(),
        response_created: 0,
        provider: Some(Arc::new(ResolvedProvider {
            protocol: target,
            upstream_path: target.upstream_path().into(),
            host: "provider.test".into(),
            port: 443,
            tls: true,
            secret: "test-provider-key".into(),
            anthropic_version: Some("2023-06-01".into()),
            messages_auth: MessagesAuth::ApiKey,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
        })),
        console: false,
        upstream_model_id: Some("models/a/b ?".into()),
        request_stream: stream,
        request_body: RequestBody::new(),
        response_body: BodyTransform::default(),
    }
}

fn header() -> RequestHeader {
    let mut header =
        RequestHeader::build("POST", b"/client?alt=json&key=client-key&trace=1", None).unwrap();
    for (name, value) in [
        ("authorization", "Bearer client"),
        ("x-api-key", "client"),
        ("x-goog-api-key", "client"),
        ("accept", "text/event-stream"),
        ("accept-encoding", "gzip"),
        ("content-type", "application/octet-stream"),
        ("content-length", "10"),
        ("content-encoding", "identity"),
        ("expect", "100-continue"),
        ("digest", "old"),
        ("content-md5", "old"),
        ("anthropic-version", "old"),
        ("anthropic-beta", "old"),
        ("openai-organization", "old"),
        ("openai-project", "old"),
        ("x-goog-api-client", "old"),
        ("x-goog-user-project", "old"),
    ] {
        header.insert_header(name, value).unwrap();
    }
    header
}

#[test]
fn four_by_four_http_metadata_matches_target_and_stream_intent() {
    for source in ALL {
        for target in ALL {
            for stream in [false, true] {
                let ctx = context(source, target, stream);
                let mut header = header();
                request::filter(&mut header, &ctx).unwrap();
                let expected = if target == Protocol::Gemini {
                    format!(
                        "/v1beta/models/a%2Fb%20%3F:{}{}",
                        if stream {
                            "streamGenerateContent"
                        } else {
                            "generateContent"
                        },
                        if source == target {
                            if stream {
                                "?trace=1&alt=sse"
                            } else {
                                "?trace=1"
                            }
                        } else if stream {
                            "?alt=sse"
                        } else {
                            ""
                        }
                    )
                } else {
                    format!(
                        "{}{}",
                        target.upstream_path(),
                        if source == target {
                            "?alt=json&key=client-key&trace=1"
                        } else {
                            ""
                        }
                    )
                };
                assert_eq!(
                    header.uri.to_string(),
                    expected,
                    "{source:?} -> {target:?}, stream={stream}"
                );
                assert_eq!(header.headers["host"], "provider.test");
                let auth = match target {
                    Protocol::Gemini => "x-goog-api-key",
                    Protocol::AnthropicMessages => "x-api-key",
                    _ => "authorization",
                };
                assert_eq!(
                    header.headers[auth],
                    if auth == "authorization" {
                        "Bearer test-provider-key"
                    } else {
                        "test-provider-key"
                    }
                );
                for name in ["authorization", "x-api-key", "x-goog-api-key"] {
                    assert_eq!(header.headers.contains_key(name), name == auth);
                }
                for name in ["content-md5", "digest"] {
                    assert!(!header.headers.contains_key(name));
                }
                if source != target {
                    assert_eq!(
                        header.headers["accept"],
                        if stream {
                            "text/event-stream"
                        } else {
                            "application/json"
                        }
                    );
                    assert_eq!(header.headers["accept-encoding"], "identity");
                    assert_eq!(header.headers["content-type"], "application/json");
                    for name in [
                        "content-length",
                        "transfer-encoding",
                        "content-encoding",
                        "expect",
                        "anthropic-beta",
                        "openai-organization",
                        "openai-project",
                        "x-goog-api-client",
                        "x-goog-user-project",
                    ] {
                        assert!(!header.headers.contains_key(name));
                    }
                } else {
                    assert_eq!(header.headers["accept"], "text/event-stream");
                    assert_eq!(header.headers["accept-encoding"], "gzip");
                    assert_eq!(header.headers["content-length"], "10");
                }
                assert_eq!(
                    header.headers.contains_key("anthropic-version"),
                    target == Protocol::AnthropicMessages || source == target
                );
                if target == Protocol::AnthropicMessages {
                    assert_eq!(header.headers["anthropic-version"], "2023-06-01");
                }
            }
        }
    }
}

#[test]
fn gemini_query_mode_is_canonical_and_client_keys_are_removed() {
    let ctx = context(Protocol::Gemini, Protocol::Gemini, true);
    let mut header = RequestHeader::build(
        "POST",
        b"/client?%61lt=json&alt=sse&%6bey=x&access%5ftoken=x&trace=1",
        None,
    )
    .unwrap();
    request::filter(&mut header, &ctx).unwrap();
    assert_eq!(header.uri.query(), Some("trace=1&alt=sse"));
}

#[tokio::test]
async fn prepared_body_length_and_provider_bearer_auth_are_sent() {
    let mut ctx = context(Protocol::OpenAiChat, Protocol::AnthropicMessages, false);
    Arc::get_mut(ctx.provider.as_mut().unwrap())
        .unwrap()
        .messages_auth = MessagesAuth::Bearer;
    ctx.request_body.set_cross_protocol(
        Protocol::OpenAiChat,
        Protocol::AnthropicMessages,
        "target-model",
    );
    let raw = Bytes::from_static(br#"{"model":"alias","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":32}"#);
    let ir = ctx
        .request_body
        .decode_cross_request(&[raw], false)
        .unwrap();
    ctx.request_body.prepare_cross_request(&ir).await.unwrap();
    let length = ctx.request_body.prepared_length().unwrap();
    let mut header = header();
    request::filter(&mut header, &ctx).unwrap();
    assert_eq!(header.headers["content-length"], length.to_string());
    assert_eq!(header.headers["authorization"], "Bearer test-provider-key");
    assert!(!header.headers.contains_key("x-api-key"));
    let mut body = None;
    ctx.request_body.push(&mut body, true).await.unwrap();
    assert_eq!(body.unwrap().len(), length);
}
