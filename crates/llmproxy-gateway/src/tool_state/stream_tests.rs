use super::*;
use crate::transform::stream::Stream;
use llmproxy_core::{adapter::protocol_codec::ResponseTarget, protocol::stream::sse::Frame};
use serde_json::json;

#[test]
fn parallel_calls_and_late_signature_share_one_restorable_group() {
    use llmproxy_core::{
        adapter::protocol_codec::ProtocolCodec,
        ir::stream::{Event, Head, Limits},
        protocol::stream::Event as Raw,
    };
    let context = Context {
        cache: Arc::new(Cache::default()),
        scope: [1; 32],
    };
    let mut decoder = Protocol::Gemini.stream_decoder(Limits::default());
    let mut capture = StreamCapture::new(context.clone(), Protocol::OpenAiChat);
    let mut published = Vec::new();
    let original: Response =
        serde_json::from_value(json!({"candidates":[{"content":{"role":"model","parts":[
            {"functionCall":{"name":"lookup","args":{"q":"a"}},"thoughtSignature":"private-first"},
            {"functionCall":{"id":"provider_2","name":"lookup","args":{"q":"b"}}}
        ]}}]}))
        .unwrap();
    for raw in [Raw::Gemini(Box::new(original.clone())), Raw::Gemini(Box::new(serde_json::from_value(json!({"candidates":[{"content":{"role":"model","parts":[{"thoughtSignature":"private-late"}]}}]})).unwrap()))] {
        let events = decoder.push(&raw).unwrap();
        capture.observe(&raw, &events).unwrap();
        for event in events {
            let (output, pending) = capture.take(event).unwrap();
            assert!(pending.is_none());
            assert!(!output.iter().any(|event| matches!(event, Event::PartStart {head: Head::Tool(_),..})), "调用组未完成前不发布引用");
            published.extend(output);
        }
    }
    let raw = Raw::Gemini(Box::new(
        serde_json::from_value(json!({"candidates":[{"finishReason":"STOP"}]})).unwrap(),
    ));
    let events = decoder.push(&raw).unwrap();
    capture.observe(&raw, &events).unwrap();
    for event in events {
        let (output, pending) = capture.take(event).unwrap();
        if let Some(pending) = pending {
            assert_eq!(pending.entries.len(), 2);
            pending.commit().unwrap();
        }
        published.extend(output);
    }
    let ids: Vec<_> = published
        .iter()
        .filter_map(|event| match event {
            Event::PartStart {
                head: Head::Tool(head),
                ..
            } => head.id.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    let mut request = Request::Gemini(
        serde_json::from_value(json!({"contents":[{"role":"model","parts":[
            {"functionCall":{"id":ids[0],"name":"lookup","args":{"q":"a"}}},
            {"functionCall":{"id":ids[1],"name":"lookup","args":{"q":"b"}}}
        ]},{"role":"user","parts":[
            {"functionResponse":{"id":ids[1],"name":"lookup","response":{"result":"OK"}}},
            {"functionResponse":{"id":ids[0],"name":"lookup","response":{"result":"OK"}}}
        ]}]}))
        .unwrap(),
    );
    context.restore(&mut request).unwrap();
    let Request::Gemini(request) = request else {
        unreachable!()
    };
    let mut expected = original.candidates.as_option().unwrap()[0]
        .content
        .as_option()
        .unwrap()
        .parts
        .clone();
    expected[1].thought_signature = O::Value("private-late".into());
    assert_eq!(request.contents[0].parts, expected);
    assert!(
        request.contents[1].parts[0]
            .function_response
            .as_option()
            .unwrap()
            .id
            .is_missing()
    );
    assert_eq!(
        request.contents[1].parts[1]
            .function_response
            .as_option()
            .unwrap()
            .id
            .as_option()
            .unwrap(),
        "provider_2"
    );
}

#[tokio::test]
async fn storage_failure_does_not_publish_signed_stream_tools() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-stream-storage-{}-{}",
        std::process::id(),
        cache::group().unwrap().1
    ));
    std::fs::create_dir(&directory).unwrap();
    let store = llmproxy_store::ProviderStore::connect(
        &format!("sqlite:{}", directory.join("unmigrated.sqlite3").display()),
        "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
    )
    .await
    .unwrap();
    // 不迁移的独立数据库使真实保存操作失败；不修改开发数据库或环境变量。
    let context = Context {
        cache: Arc::new(Cache::database(store)),
        scope: [1; 32],
    };
    let mut stream = Stream::new(
        Protocol::Gemini,
        Protocol::OpenAiChat,
        &ResponseTarget {
            model: "m",
            id: "r",
            created: 1,
        },
        Some(context),
    )
    .unwrap();
    let frame = |value| Frame {
        event: None,
        data: serde_json::to_vec(&value).unwrap(),
    };
    let text = stream
        .convert(&frame(
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]}}]}),
        ))
        .await
        .unwrap();
    assert!(
        text.iter()
            .any(|b| String::from_utf8_lossy(b).contains("hi"))
    );
    let pending=stream.convert(&frame(json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{}},"thoughtSignature":"private-signature"}]}}]}))).await.unwrap();
    assert!(
        pending
            .iter()
            .all(|b| !String::from_utf8_lossy(b).contains("tool_calls"))
    );
    let error = stream
        .convert(&frame(json!({"candidates":[{"finishReason":"STOP"}]})))
        .await
        .unwrap_err();
    assert_eq!(error.etype(), &pingora::ErrorType::HTTPStatus(503));
    drop(stream);
    std::fs::remove_dir_all(directory).unwrap();
}
