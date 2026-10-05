//! 短期工具状态的约束回归，不调用真实模型。

use super::*;
use llmproxy_core::adapter::protocol_codec::ProtocolCodec;
use serde_json::json;

/// 并行调用只有第一个带签名；原调用没有 ID，也必须原样恢复缺失。
fn fixture() -> Response {
    serde_json::from_value(json!({"candidates":[{"content":{"role":"model","parts":[
        {"functionCall":{"name":"lookup","args":{"q":"a"}},"thoughtSignature":"private-signature"},
        {"functionCall":{"id":"provider_2","name":"lookup","args":{"q":"b"}}}
    ]},"finishReason":"STOP"}]}))
    .unwrap()
}

fn context() -> Context {
    Context {
        cache: Arc::new(Cache::default()),
        scope: [1; 32],
    }
}

/// 测试引用以原生 Gemini 形式表示，HTTP 集成再覆盖三种客户端的往返。
fn prepare(context: &Context) -> Request {
    let raw = fixture();
    let mut ir = Protocol::Gemini
        .decode_response(&llmproxy_core::protocol::Response::Gemini(Box::new(
            raw.clone(),
        )))
        .unwrap();
    let pending = context
        .capture(&raw, &mut ir, Protocol::OpenAiChat)
        .unwrap();
    let ids: Vec<_> = ir.messages[0]
        .parts
        .iter()
        .filter_map(|part| match &part.kind {
            PartKind::ToolCall(call) => call.id.clone(),
            _ => None,
        })
        .collect();
    assert!(ids.iter().all(|id| id.len() <= 64));
    pending.commit().unwrap();
    Request::Gemini(
        serde_json::from_value(json!({"contents":[
            {"role":"model","parts":[
                {"functionCall":{"id":ids[0],"name":"lookup","args":{"q":"a"}}},
                {"functionCall":{"id":ids[1],"name":"lookup","args":{"q":"b"}}}
            ]},
            {"role":"user","parts":[
                {"functionResponse":{"id":ids[0],"name":"lookup","response":{"result":"OK"}}},
                {"functionResponse":{"id":ids[1],"name":"lookup","response":{"result":"OK"}}}
            ]}
        ]}))
        .unwrap(),
    )
}

#[test]
fn restores_exact_parallel_parts_and_result_ids_without_consuming_history() {
    let context = context();
    let request = prepare(&context);
    for _ in 0..2 {
        let mut request = request.clone();
        context.restore(&mut request).unwrap();
        let Request::Gemini(request) = request else {
            unreachable!()
        };
        assert_eq!(
            request.contents[0].parts,
            fixture().candidates.as_option().unwrap()[0]
                .content
                .as_option()
                .unwrap()
                .parts
        );
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
}

#[test]
fn parallel_results_return_to_original_call_order_even_when_client_finishes_out_of_order() {
    let context = context();
    let mut request = prepare(&context);
    let Request::Gemini(body) = &mut request else {
        unreachable!()
    };
    body.contents[1].parts.swap(0, 1);
    context.restore(&mut request).unwrap();
    let Request::Gemini(body) = request else {
        unreachable!()
    };
    assert!(
        body.contents[1].parts[0]
            .function_response
            .as_option()
            .unwrap()
            .id
            .is_missing()
    );
    assert_eq!(
        body.contents[1].parts[1]
            .function_response
            .as_option()
            .unwrap()
            .id
            .as_option()
            .unwrap(),
        "provider_2"
    );
}

#[test]
fn rejects_modified_calls_reordered_or_incomplete_groups_and_scope_changes() {
    let context = context();
    let request = prepare(&context);
    let mut modified = request.clone();
    let Request::Gemini(body) = &mut modified else {
        unreachable!()
    };
    if let O::Value(call) = &mut body.contents[0].parts[0].function_call {
        call.args = O::Value(serde_json::from_value(json!({"q":"changed"})).unwrap());
    }
    assert!(context.restore(&mut modified).is_err());
    let mut reordered = request.clone();
    let Request::Gemini(body) = &mut reordered else {
        unreachable!()
    };
    body.contents[0].parts.swap(0, 1);
    assert!(context.restore(&mut reordered).is_err());
    let mut incomplete = request.clone();
    let Request::Gemini(body) = &mut incomplete else {
        unreachable!()
    };
    body.contents[0].parts.pop();
    assert!(context.restore(&mut incomplete).is_err());
    let other = Context {
        cache: context.cache.clone(),
        scope: [2; 32],
    };
    assert!(other.restore(&mut request.clone()).is_err());
}

#[test]
fn expired_missing_and_unpublished_state_never_fabricates_signatures() {
    let context = context();
    let request = prepare(&context);
    cache::expire(&context.cache);
    assert!(context.restore(&mut request.clone()).is_err());
    let other = self::context();
    assert!(other.restore(&mut request.clone()).is_err());
    let raw = fixture();
    let mut ir = Protocol::Gemini
        .decode_response(&llmproxy_core::protocol::Response::Gemini(Box::new(
            raw.clone(),
        )))
        .unwrap();
    let pending = other.capture(&raw, &mut ir, Protocol::OpenAiChat).unwrap();
    assert_eq!(pending.entries.len(), 2);
    assert!(
        other
            .cache
            .get(&pending.entries[0].0, &other.scope)
            .is_err()
    );
}

#[test]
fn oversized_pending_state_is_rejected_without_eviction() {
    let context = context();
    let request = prepare(&context);
    let raw = fixture();
    let mut ir = Protocol::Gemini
        .decode_response(&llmproxy_core::protocol::Response::Gemini(Box::new(
            raw.clone(),
        )))
        .unwrap();
    let mut pending = context
        .capture(&raw, &mut ir, Protocol::OpenAiChat)
        .unwrap();
    pending.entries[0].1.bytes = 16 * 1024 * 1024;
    assert!(pending.commit().is_err());
    context.restore(&mut request.clone()).unwrap();
}
