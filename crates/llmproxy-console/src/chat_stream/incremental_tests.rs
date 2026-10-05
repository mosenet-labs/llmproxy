use super::incremental::Reply;
use llmproxy_core::protocol::Protocol;
use serde_json::{Value, json};

fn packet(value: Value) -> Vec<u8> {
    format!("data: {}\n\n", serde_json::to_string(&value).unwrap()).into_bytes()
}

#[test]
fn tool_only_streams_display_args_and_usage_in_four_protocols() {
    for (protocol, values) in [
        (
            Protocol::OpenAiChat,
            vec![
                json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"a","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},"finish_reason":"tool_calls"}]}),
                json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":4}}}),
            ],
        ),
        (
            Protocol::OpenAiResponses,
            vec![
                json!({"type":"response.created","sequence_number":0,"response":{"id":"r","model":"m","created_at":1,"object":"response","output":[]}}),
                json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"function_call","id":"f","call_id":"a","name":"lookup","arguments":""}}),
                json!({"type":"response.output_item.done","sequence_number":2,"output_index":0,"item":{"type":"function_call","id":"f","call_id":"a","name":"lookup","arguments":"{\"q\":1}"}}),
                json!({"type":"response.completed","sequence_number":3,"response":{"id":"r","model":"m","created_at":1,"object":"response","status":"completed","output":[{"type":"function_call","id":"f","call_id":"a","name":"lookup","arguments":"{\"q\":1}"}],"usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}}}}),
            ],
        ),
        (
            Protocol::AnthropicMessages,
            vec![
                json!({"type":"message_start","message":{"id":"m","type":"message","model":"m","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":6,"cache_read_input_tokens":4,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"lookup","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":1}"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
                json!({"type":"message_stop"}),
            ],
        ),
        (
            Protocol::Gemini,
            vec![
                json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":"a","name":"lookup","args":{"q":1}},"thoughtSignature":"private"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13,"cachedContentTokenCount":4}}),
            ],
        ),
    ] {
        let mut reply = Reply::new(protocol);
        let mut updates = 0;
        for value in values {
            for bytes in packet(value).chunks(2) {
                reply.push(bytes, &mut |_| updates += 1).unwrap();
            }
        }
        if protocol == Protocol::OpenAiChat {
            reply
                .push(b"data: [DONE]\n\n", &mut |_| updates += 1)
                .unwrap();
        }
        let reply = reply.finish().unwrap();
        assert!(updates > 0);
        assert!(reply.content.is_empty());
        assert_eq!(reply.parts.len(), 1);
        assert_eq!(reply.parts[0].title, "工具调用 · lookup");
        assert_eq!(
            serde_json::from_str::<Value>(&reply.parts[0].text).unwrap(),
            json!({"q":1})
        );
        let usage = reply.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.total_tokens, Some(13));
        assert_eq!(usage.cache.read_input_tokens, Some(4));
        assert!(!format!("{:?}", reply.parts).contains("private"));
    }
}

#[test]
fn partial_text_updates_immediately_and_truncation_does_not_complete() {
    let mut reply = Reply::new(Protocol::OpenAiChat);
    let mut text = String::new();
    reply.push(&packet(json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"你好","reasoning_content":"分析"}}]})),&mut |r|text=r.content.clone()).unwrap();
    assert_eq!(text, "你好");
    assert!(reply.finish().is_err());
    let mut reply = Reply::new(Protocol::OpenAiChat);
    assert!(
        reply
            .push(
                b"data: {\"error\":{\"message\":\"private-provider-detail\"}}\n\n",
                &mut |_| {}
            )
            .unwrap_err()
            .contains("生成失败")
    );
}
