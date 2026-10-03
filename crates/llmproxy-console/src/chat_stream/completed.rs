//! 控制台非流式 HTTP 边界：字节只在此反序列化为协议类型。
use super::ChatReply;
use llmproxy_core::{
    adapter::protocol_codec::ProtocolCodec,
    protocol::{
        OptionalNullable as O, Protocol, Response,
        messages::response::message::{ContentBlock, KnownContentBlock},
        responses::response::body::OutputItem,
    },
};

/// 用具体协议读取显示内容，通过整体 IR 取得相同口径的 cache / usage。
pub(super) fn decode(protocol: Protocol, bytes: &[u8]) -> Result<ChatReply, String> {
    let parsed: serde_json::Result<Response> = (|| {
        Ok(match protocol {
            Protocol::OpenAiChat => Response::Chat(Box::new(serde_json::from_slice(bytes)?)),
            Protocol::OpenAiResponses => {
                Response::Responses(Box::new(serde_json::from_slice(bytes)?))
            }
            Protocol::AnthropicMessages => {
                Response::Messages(Box::new(serde_json::from_slice(bytes)?))
            }
            Protocol::Gemini => Response::Gemini(Box::new(serde_json::from_slice(bytes)?)),
        })
    })();
    let body = parsed.map_err(|_| "无法解析上游非流式响应".to_owned())?;
    let mut reply = ChatReply::default();
    match &body {
        Response::Chat(body) => {
            if let Some(choice) = body.choices.iter().min_by_key(|c| c.index) {
                reply.content = choice
                    .message
                    .content
                    .as_option()
                    .cloned()
                    .unwrap_or_default();
                if let Some(refusal) = choice.message.refusal.as_option() {
                    reply.content.push_str(refusal);
                }
                reply.thinking = choice
                    .message
                    .extra
                    .get("reasoning_content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into();
                if reply.content.is_empty() {
                    reply.content = finish_text(&choice.finish_reason).into();
                }
            }
        }
        Response::Responses(body) => {
            if let Some(error) = body.error.as_option() {
                return Err(error.message.clone());
            }
            for item in &body.output {
                match item {
                    OutputItem::Message(message) => {
                        for part in &message.content {
                            use llmproxy_core::protocol::responses::request::OutputPart;
                            match part {
                                OutputPart::OutputText { text, .. } => reply.content.push_str(text),
                                OutputPart::Refusal { refusal, .. } => {
                                    reply.content.push_str(refusal)
                                }
                            }
                        }
                    }
                    OutputItem::Other(item)
                        if item.get("type").and_then(serde_json::Value::as_str)
                            == Some("reasoning") =>
                    {
                        for part in item
                            .get("summary")
                            .and_then(serde_json::Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            if let Some(text) = part.get("text").and_then(serde_json::Value::as_str)
                            {
                                reply.summary.push_str(text);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if reply.content.is_empty()
                && let Some(details) = body.incomplete_details.as_option()
            {
                reply.content = finish_text(&details.reason).into();
            }
        }
        Response::Messages(body) => {
            for part in &body.content {
                match part {
                    ContentBlock::Known(KnownContentBlock::Text { text, .. }) => {
                        reply.content.push_str(text)
                    }
                    ContentBlock::Known(KnownContentBlock::Thinking { thinking, .. }) => {
                        reply.thinking.push_str(thinking)
                    }
                    _ => {}
                }
            }
            if reply.content.is_empty()
                && let Some(reason) = body.stop_reason.as_option()
            {
                reply.content = finish_text(reason).into();
            }
        }
        Response::Gemini(body) => {
            if let Some(candidate) = body.candidates.as_option().and_then(|v| {
                v.iter()
                    .min_by_key(|c| c.index.as_option().copied().unwrap_or(0))
            }) {
                if let Some(content) = candidate.content.as_option() {
                    for part in &content.parts {
                        if let Some(text) = part.text.as_option() {
                            if part.thought == O::Value(true) {
                                reply.thinking.push_str(text);
                            } else {
                                reply.content.push_str(text);
                            }
                        }
                    }
                }
                if reply.content.is_empty()
                    && let Some(reason) = candidate.finish_reason.as_option()
                {
                    reply.content = finish_text(reason).into();
                }
            }
            if reply.content.is_empty() && body.prompt_feedback.as_option().is_some() {
                reply.content = "输入被 Provider 阻止，未生成回复".into();
            }
        }
    }
    reply.usage = protocol
        .decode_response(&body)
        .map_err(|_| "无法读取响应用量".to_owned())?
        .usage;
    if reply.content.is_empty() && reply.thinking.is_empty() && reply.summary.is_empty() {
        return Err("上游未返回可显示的文本内容".into());
    }
    Ok(reply)
}
/// 无正文的截断和拒绝仍应有可理解的页面提示。
fn finish_text(reason: &str) -> &'static str {
    match reason {
        "length" | "max_tokens" | "max_output_tokens" | "MAX_TOKENS" => {
            "已达到输出上限，本次没有可显示的回答"
        }
        "content_filter" | "refusal" | "SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" => {
            "本次回复被拒绝或过滤"
        }
        _ => "",
    }
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
}
