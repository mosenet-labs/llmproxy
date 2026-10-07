//! 控制台非流式 HTTP 边界：字节只在此反序列化为协议类型。
use super::ChatReply;
use llmproxy_core::{
    adapter::protocol_codec::ProtocolCodec,
    protocol::{Protocol, Response},
};

/// HTTP 边界解析协议类型，再通过整体 IR 取得正文、展示块和统一 cache / usage。
pub(super) fn decode(protocol: Protocol, bytes: &[u8]) -> Result<ChatReply, String> {
    let body = llmproxy_core::protocol::wire::decode_response(protocol, bytes)
        .map_err(|_| "无法解析上游非流式响应".to_owned())?;
    let ir = protocol
        .decode_response(&body)
        .map_err(|_| "无法解码上游非流式响应".to_owned())?;
    use llmproxy_core::ir::response::Status;
    match ir.status {
        Status::Failed => return Err("Provider 生成失败".into()),
        Status::Cancelled => return Err("Provider 已取消本次生成".into()),
        Status::InProgress => return Err("Provider 尚未完成生成，当前对话不支持后台轮询".into()),
        Status::Unknown => return Err("Provider 返回了未知生成状态".into()),
        Status::Completed | Status::Incomplete => {}
    }
    let mut reply = ChatReply {
        status: Some(ir.status),
        ..Default::default()
    };
    super::display::read(&ir, &mut reply);
    if let Response::Chat(body) = &body
        && let Some(audio) = body
            .choices
            .iter()
            .min_by_key(|choice| choice.index)
            .and_then(|choice| choice.message.audio.as_option())
    {
        super::display::chat_audio(audio, &mut reply)?;
    }
    if reply.content.is_empty()
        && reply.parts.is_empty()
        && let Some(candidate) = ir.candidates.iter().min_by_key(|candidate| candidate.index)
    {
        use llmproxy_core::ir::response::FinishReason;
        reply.content = match candidate.finish_reason {
            FinishReason::Length => "已达到输出上限，本次没有可显示的回答",
            FinishReason::Filtered | FinishReason::Refusal => "本次回复被拒绝或过滤",
            _ => "",
        }
        .into();
    }
    reply.history = super::history::Content::response(&ir);
    reply.usage = ir.usage;
    reply.model = ir.model;
    if reply.content.is_empty()
        && reply.thinking.is_empty()
        && reply.summary.is_empty()
        && reply.parts.is_empty()
    {
        return Err("上游未返回可显示的文本内容".into());
    }
    Ok(reply)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_reply_reads_text_and_cache_usage() {
        let reply=decode(Protocol::AnthropicMessages,br#"{"id":"m","type":"message","role":"assistant","model":"m","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":20,"cache_read_input_tokens":50,"output_tokens":5}}"#).unwrap();
        assert_eq!(reply.content, "hello");
        let usage = reply.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(70));
        assert_eq!(usage.cache.read_input_tokens, Some(50));
    }

    #[test]
    fn incomplete_provider_states_do_not_enter_chat_history() {
        for status in ["failed", "cancelled", "queued", "in_progress"] {
            let body = serde_json::json!({"id":"r","created_at":1,"model":"m","object":"response","status":status,"output":[],"error":if status == "failed" {serde_json::json!({"code":"server_error","message":"private-provider-detail"})} else {serde_json::Value::Null}});
            let error = decode(
                Protocol::OpenAiResponses,
                &serde_json::to_vec(&body).unwrap(),
            )
            .unwrap_err();
            assert!(!error.contains("private-provider-detail"));
        }
    }

    #[test]
    fn tool_only_replies_are_visible_in_all_four_protocols() {
        use serde_json::json;
        for (protocol, body) in [
            (
                Protocol::OpenAiChat,
                json!({"id":"c","created":1,"model":"m","object":"chat.completion","choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"type":"function","id":"call","function":{"name":"lookup","arguments":"{\"q\":\"test\"}"}}]}}]}),
            ),
            (
                Protocol::OpenAiResponses,
                json!({"id":"r","created_at":1,"model":"m","object":"response","status":"completed","output":[{"type":"function_call","call_id":"call","name":"lookup","arguments":"{\"q\":\"test\"}","status":"completed"}]}),
            ),
            (
                Protocol::AnthropicMessages,
                json!({"id":"m","type":"message","role":"assistant","model":"m","content":[{"type":"tool_use","id":"call","name":"lookup","input":{"q":"test"}}],"stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1}}),
            ),
            (
                Protocol::Gemini,
                json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"id":"call","name":"lookup","args":{"q":"test"}},"thoughtSignature":"private-signature"}]},"finishReason":"STOP"}]}),
            ),
        ] {
            let reply = decode(protocol, &serde_json::to_vec(&body).unwrap()).unwrap();
            assert!(reply.content.is_empty());
            assert_eq!(reply.parts.len(), 1);
            assert_eq!(reply.parts[0].title, "工具调用 · lookup");
            assert!(reply.parts[0].text.contains("test"));
            assert!(!format!("{:?}", reply.parts).contains("private-signature"));
            assert!(format!("{:?}", reply.history).contains("lookup"));
            assert!(format!("{:?}", reply.history).contains("test"));
            if protocol == Protocol::Gemini {
                assert!(format!("{:?}", reply.history).contains("private-signature"));
            }
        }
    }

    #[test]
    fn lowest_candidate_keeps_text_media_and_tool_display_order() {
        let body = serde_json::json!({"candidates":[
            {"index":4,"content":{"role":"model","parts":[{"text":"discarded"}]}},
            {"index":1,"content":{"role":"model","parts":[{"text":"before"},{"inlineData":{"mimeType":"image/png","data":"YQ=="}},{"functionCall":{"name":"lookup","args":{}}},{"text":"after"}]}}
        ]});
        let reply = decode(Protocol::Gemini, &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(reply.content, "beforeafter");
        assert_eq!(reply.parts.len(), 4);
        assert_eq!(reply.parts[0].text, "before");
        assert!(reply.parts[1].media.as_ref().unwrap().preview);
        assert_eq!(reply.parts[2].title, "工具调用 · lookup");
        assert_eq!(reply.parts[3].text, "after");
        let parts = &reply.history.messages[0].parts;
        assert_eq!(parts.len(), 4);
        assert!(matches!(
            parts[1].kind,
            llmproxy_core::ir::message::PartKind::Media(_)
        ));
        assert!(matches!(
            parts[2].kind,
            llmproxy_core::ir::message::PartKind::ToolCall(_)
        ));
        assert!(!format!("{:?}", reply.history).contains("discarded"));
    }

    #[test]
    fn audio_and_server_outputs_do_not_expose_private_metadata() {
        let body = serde_json::json!({"id":"c","created":1,"model":"m","object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","audio":{"id":"private-audio","data":"YQ==","expires_at":100,"transcript":"hello"}}}]});
        let reply = decode(Protocol::OpenAiChat, &serde_json::to_vec(&body).unwrap()).unwrap();
        assert!(!reply.parts[0].media.as_ref().unwrap().preview);
        assert_eq!(
            reply.parts[0].media.as_ref().unwrap().uri,
            "data:application/octet-stream;base64,YQ=="
        );
        assert_eq!(reply.parts[1].text, "hello");
        assert!(!format!("{:?}", reply.parts).contains("private-audio"));
        let body = serde_json::json!({"id":"r","created_at":1,"model":"m","object":"response","status":"completed","output":[{"type":"code_interpreter_call","id":"private-call","container_id":"private-container","status":"completed","code":"print(42)","outputs":[{"type":"logs","logs":"42"}]}]});
        let reply = decode(
            Protocol::OpenAiResponses,
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap();
        assert_eq!(reply.parts[0].title, "服务端执行结果");
        assert!(reply.parts[0].text.contains("42"));
        assert!(!format!("{:?}", reply.parts).contains("private"));
    }

    #[test]
    fn nonstream_audio_recognizes_containers_and_rejects_invalid_base64() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut body = serde_json::json!({"id":"c","created":1,"model":"m","object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","audio":{"id":"audio","data":STANDARD.encode(b"RIFF\0\0\0\0WAVE"),"expires_at":100,"transcript":"transcript"}}}]});
        let reply = decode(Protocol::OpenAiChat, &serde_json::to_vec(&body).unwrap()).unwrap();
        let media = reply.parts[0].media.as_ref().unwrap();
        assert!(media.preview);
        assert!(media.uri.starts_with("data:audio/wav;base64,"));
        assert_eq!(reply.parts[1].text, "transcript");
        body["choices"][0]["message"]["audio"]["data"] = "invalid-base64".into();
        assert_eq!(
            decode(Protocol::OpenAiChat, &serde_json::to_vec(&body).unwrap()).unwrap_err(),
            "上游音频数据无效"
        );
    }

    #[test]
    fn active_media_types_and_non_public_schemes_are_not_rendered() {
        let body = serde_json::json!({"candidates":[{"content":{"role":"model","parts":[
            {"inlineData":{"mimeType":"image/svg+xml","data":"YQ=="}},
            {"fileData":{"mimeType":"image/png","fileUri":"javascript:alert(1)"}},
            {"fileData":{"mimeType":"image/png","fileUri":"https://user:password@example.com/a.png"}}
        ]}}]});
        let reply = decode(Protocol::Gemini, &serde_json::to_vec(&body).unwrap()).unwrap();
        let media = reply.parts[0].media.as_ref().unwrap();
        assert!(!media.preview);
        assert_eq!(media.uri, "data:application/octet-stream;base64,YQ==");
        assert!(reply.parts[1].media.is_none());
        assert!(reply.parts[2].media.is_none());
        assert!(!format!("{:?}", reply.parts).contains("password"));
    }
}
