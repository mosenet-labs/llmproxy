use llmproxy_core::protocol::Protocol;
use reqwest::Client;
use serde_json::{Value, json};
use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};

const MAX_REPLY_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
pub(crate) struct ChatReply {
    pub content: String,
    pub thinking: String,
    pub summary: String,
}

impl ChatReply {
    pub fn visible_thinking(&self) -> &str {
        if self.thinking.is_empty() {
            &self.summary
        } else {
            &self.thinking
        }
    }
}

pub fn request_body(protocol: Protocol, alias: &str, history: &[ChatMessage]) -> Value {
    let messages: Vec<_> = history
        .iter()
        .filter(|message| message.status == ChatMessageStatus::Complete)
        .map(|message| {
            json!({
                "role": if message.role == ChatBubbleRole::User { "user" } else { "assistant" },
                "content": message.content,
            })
        })
        .collect();
    match protocol {
        Protocol::OpenAiChat => json!({ "model": alias, "stream": true, "messages": messages }),
        Protocol::OpenAiResponses => json!({ "model": alias, "stream": true, "input": messages }),
        Protocol::AnthropicMessages => {
            json!({ "model": alias, "stream": true, "max_tokens": 2048, "messages": messages })
        }
    }
}

fn error_message(value: &Value) -> String {
    value
        .as_str()
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("上游请求失败")
        .to_owned()
}

fn event(protocol: Protocol, packet: &str) -> Result<(ChatReply, bool), String> {
    let mut kind = "";
    let mut data = Vec::new();
    for line in packet.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("event:") {
            kind = value.trim();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start());
        }
    }
    if data.is_empty() {
        return Ok((ChatReply::default(), false));
    }
    let data = data.join("\n");
    if data.trim() == "[DONE]" {
        return Ok((ChatReply::default(), true));
    }
    let value: Value = serde_json::from_str(&data).map_err(|_| "无法解析上游流式响应")?;
    let kind = value.get("type").and_then(Value::as_str).unwrap_or(kind);
    if let Some(failure) = value.get("error").or_else(|| {
        (kind == "response.failed")
            .then(|| value.pointer("/response/error"))
            .flatten()
    }) {
        return Err(error_message(failure));
    }
    if kind == "error" || kind == "response.incomplete" {
        return Err(error_message(
            value
                .pointer("/response/incomplete_details")
                .unwrap_or(&value),
        ));
    }
    let (content, thinking, summary, done) = match protocol {
        Protocol::OpenAiChat => (
            value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str),
            value
                .pointer("/choices/0/delta/reasoning_content")
                .and_then(Value::as_str),
            None,
            false,
        ),
        Protocol::OpenAiResponses => (
            (kind == "response.output_text.delta")
                .then(|| value.get("delta").and_then(Value::as_str))
                .flatten(),
            (kind == "response.reasoning_text.delta")
                .then(|| value.get("delta").and_then(Value::as_str))
                .flatten(),
            (kind == "response.reasoning_summary_text.delta")
                .then(|| value.get("delta").and_then(Value::as_str))
                .flatten(),
            kind == "response.completed",
        ),
        Protocol::AnthropicMessages => (
            match kind {
                "content_block_delta"
                    if value.pointer("/delta/type").and_then(Value::as_str)
                        == Some("text_delta") =>
                {
                    value.pointer("/delta/text").and_then(Value::as_str)
                }
                "content_block_start"
                    if value.pointer("/content_block/type").and_then(Value::as_str)
                        == Some("text") =>
                {
                    value.pointer("/content_block/text").and_then(Value::as_str)
                }
                _ => None,
            },
            match kind {
                "content_block_delta"
                    if value.pointer("/delta/type").and_then(Value::as_str)
                        == Some("thinking_delta") =>
                {
                    value.pointer("/delta/thinking").and_then(Value::as_str)
                }
                "content_block_start"
                    if value.pointer("/content_block/type").and_then(Value::as_str)
                        == Some("thinking") =>
                {
                    value
                        .pointer("/content_block/thinking")
                        .and_then(Value::as_str)
                }
                _ => None,
            },
            None,
            kind == "message_stop",
        ),
    };
    Ok((
        ChatReply {
            content: content.unwrap_or_default().to_owned(),
            thinking: thinking.unwrap_or_default().to_owned(),
            summary: summary.unwrap_or_default().to_owned(),
        },
        done,
    ))
}

fn completed_text(protocol: Protocol, value: &Value) -> String {
    match protocol {
        Protocol::OpenAiChat => value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Protocol::OpenAiResponses => value
            .get("output_text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                value["output"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|item| item["content"].as_array().into_iter().flatten())
                    .filter(|item| item["type"] == "output_text")
                    .filter_map(|item| item["text"].as_str())
                    .collect()
            }),
        Protocol::AnthropicMessages => value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "text")
            .filter_map(|item| item["text"].as_str())
            .collect(),
    }
}

fn completed_thinking(protocol: Protocol, value: &Value) -> String {
    match protocol {
        Protocol::OpenAiChat => value
            .pointer("/choices/0/message/reasoning_content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Protocol::OpenAiResponses => {
            let parts: Vec<_> = value["output"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| item["type"] == "reasoning")
                .collect();
            let raw = parts
                .iter()
                .flat_map(|item| item["content"].as_array().into_iter().flatten())
                .filter(|part| part["type"] == "reasoning_text")
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !raw.is_empty() {
                raw
            } else {
                parts
                    .iter()
                    .flat_map(|item| item["summary"].as_array().into_iter().flatten())
                    .filter(|part| part["type"] == "summary_text")
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        Protocol::AnthropicMessages => value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "thinking")
            .filter_map(|item| item["thinking"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    (0..bytes.len()).find_map(|index| {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            Some((index, 4))
        } else if bytes[index..].starts_with(b"\n\n") {
            Some((index, 2))
        } else {
            None
        }
    })
}

pub async fn stream_reply(
    client: &Client,
    gateway_origin: &str,
    protocol: Protocol,
    alias: &str,
    history: &[ChatMessage],
    mut on_update: impl FnMut(&ChatReply),
) -> Result<ChatReply, String> {
    let response = client
        .post(format!("{gateway_origin}{}", protocol.upstream_path()))
        .header("accept", "text/event-stream")
        .json(&request_body(protocol, alias, history))
        .send()
        .await
        .map_err(|error| format!("无法连接网关：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.get("error").map(error_message))
            .or_else(|| {
                content_type
                    .contains("text/plain")
                    .then(|| body.chars().take(240).collect())
            })
            .unwrap_or_default();
        return Err(if detail.is_empty() {
            format!("HTTP {}", status.as_u16())
        } else {
            format!("HTTP {}：{detail}", status.as_u16())
        });
    }
    if response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"))
    {
        let value: Value = response.json().await.map_err(|_| "无法解析上游响应")?;
        let reply = ChatReply {
            content: completed_text(protocol, &value),
            thinking: completed_thinking(protocol, &value),
            summary: String::new(),
        };
        if reply.content.is_empty() {
            return Err("上游未返回文本内容".to_owned());
        }
        on_update(&reply);
        return Ok(reply);
    }

    let mut response = response;
    let mut pending = Vec::new();
    let mut output = ChatReply::default();
    let mut done = false;
    while !done {
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("读取上游响应失败：{error}"))?
        else {
            break;
        };
        pending.extend_from_slice(&chunk);
        if pending.len() > MAX_REPLY_BYTES {
            return Err("上游回复过长".to_owned());
        }
        while let Some((index, separator)) = boundary(&pending) {
            let packet = std::str::from_utf8(&pending[..index])
                .map_err(|_| "上游流式响应不是有效的 UTF-8")?;
            let (delta, finished) = event(protocol, packet)?;
            if output.content.len()
                + output.thinking.len()
                + output.summary.len()
                + delta.content.len()
                + delta.thinking.len()
                + delta.summary.len()
                > MAX_REPLY_BYTES
            {
                return Err("上游回复过长".to_owned());
            }
            if !delta.content.is_empty() || !delta.thinking.is_empty() || !delta.summary.is_empty()
            {
                output.content.push_str(&delta.content);
                output.thinking.push_str(&delta.thinking);
                output.summary.push_str(&delta.summary);
                on_update(&output);
            }
            done = finished;
            pending.drain(..index + separator);
            if done {
                break;
            }
        }
    }
    if !done && !pending.is_empty() {
        let packet = std::str::from_utf8(&pending).map_err(|_| "上游流式响应不是有效的 UTF-8")?;
        let (delta, _) = event(protocol, packet)?;
        if output.content.len()
            + output.thinking.len()
            + output.summary.len()
            + delta.content.len()
            + delta.thinking.len()
            + delta.summary.len()
            > MAX_REPLY_BYTES
        {
            return Err("上游回复过长".to_owned());
        }
        output.content.push_str(&delta.content);
        output.thinking.push_str(&delta.thinking);
        output.summary.push_str(&delta.summary);
        if !delta.content.is_empty() || !delta.thinking.is_empty() || !delta.summary.is_empty() {
            on_update(&output);
        }
    }
    if output.content.is_empty() {
        return Err("上游未返回文本内容".to_owned());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{boundary, completed_text, completed_thinking, event, request_body};
    use llmproxy_core::protocol::Protocol;
    use serde_json::json;
    use topcoat_ant_design::{ChatBubbleRole, ChatMessage, ChatMessageStatus};

    #[test]
    fn request_bodies_match_each_protocol() {
        let history = vec![
            ChatMessage::new("1", ChatBubbleRole::User, ChatMessageStatus::Complete, "hi"),
            ChatMessage::new(
                "2",
                ChatBubbleRole::Assistant,
                ChatMessageStatus::Sending,
                "",
            ),
        ];
        let chat = request_body(Protocol::OpenAiChat, "alias", &history);
        assert_eq!(
            chat["messages"],
            json!([{ "role": "user", "content": "hi" }])
        );
        assert_eq!(chat["model"], "alias");
        assert_eq!(
            request_body(Protocol::OpenAiResponses, "alias", &history)["input"],
            chat["messages"]
        );
        assert_eq!(
            request_body(Protocol::AnthropicMessages, "alias", &history)["max_tokens"],
            2048
        );
    }

    #[test]
    fn extracts_streamed_text_and_errors() {
        assert_eq!(
            event(
                Protocol::OpenAiChat,
                "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}"
            )
            .unwrap()
            .0
            .content,
            "你好"
        );
        assert_eq!(
            event(
                Protocol::OpenAiResponses,
                "event: response.output_text.delta\ndata: {\"delta\":\"A\"}"
            )
            .unwrap()
            .0
            .content,
            "A"
        );
        assert_eq!(event(Protocol::AnthropicMessages, "event: content_block_delta\ndata: {\"delta\":{\"type\":\"text_delta\",\"text\":\"B\"}}").unwrap().0.content, "B");
        assert!(
            event(
                Protocol::AnthropicMessages,
                "event: error\ndata: {\"error\":{\"message\":\"rate limit\"}}"
            )
            .unwrap_err()
            .contains("rate limit")
        );
        assert!(event(Protocol::OpenAiChat, "data: [DONE]").unwrap().1);
        assert_eq!(boundary(b"data: 1\r\n\r\ndata: 2\n\n"), Some((7, 4)));
    }

    #[test]
    fn extracts_streamed_thinking_for_each_protocol() {
        assert_eq!(
            event(
                Protocol::OpenAiChat,
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"分析\"}}]}"
            )
            .unwrap()
            .0
            .thinking,
            "分析"
        );
        assert_eq!(
            event(
                Protocol::OpenAiResponses,
                "event: response.reasoning_text.delta\ndata: {\"delta\":\"推理\"}"
            )
            .unwrap()
            .0
            .thinking,
            "推理"
        );
        assert_eq!(
            event(
                Protocol::OpenAiResponses,
                "event: response.reasoning_summary_text.delta\ndata: {\"delta\":\"摘要\"}"
            )
            .unwrap()
            .0
            .visible_thinking(),
            "摘要"
        );
        assert_eq!(
            event(
                Protocol::AnthropicMessages,
                "event: content_block_delta\ndata: {\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"思考\"}}"
            )
            .unwrap()
            .0
            .thinking,
            "思考"
        );
        assert!(
            event(
                Protocol::AnthropicMessages,
                "event: content_block_delta\ndata: {\"delta\":{\"type\":\"signature_delta\",\"signature\":\"secret\"}}"
            )
            .unwrap()
            .0
            .thinking
            .is_empty()
        );
    }

    #[test]
    fn extracts_non_streaming_fallback() {
        assert_eq!(
            completed_text(
                Protocol::OpenAiChat,
                &json!({"choices":[{"message":{"content":"chat"}}]})
            ),
            "chat"
        );
        assert_eq!(
            completed_text(
                Protocol::OpenAiResponses,
                &json!({"output":[{"content":[{"type":"output_text","text":"reply"}]}]})
            ),
            "reply"
        );
        assert_eq!(
            completed_text(
                Protocol::AnthropicMessages,
                &json!({"content":[{"type":"text","text":"message"}]})
            ),
            "message"
        );
        assert_eq!(
            completed_thinking(
                Protocol::OpenAiChat,
                &json!({"choices":[{"message":{"reasoning_content":"chat thought"}}]})
            ),
            "chat thought"
        );
        assert_eq!(
            completed_thinking(
                Protocol::OpenAiResponses,
                &json!({"output":[{"type":"reasoning","content":[{"type":"reasoning_text","text":"response thought"}]}]})
            ),
            "response thought"
        );
        assert_eq!(
            completed_thinking(
                Protocol::OpenAiResponses,
                &json!({"output":[{"type":"reasoning","summary":[{"type":"summary_text","text":"response summary"}]}]})
            ),
            "response summary"
        );
        assert_eq!(
            completed_thinking(
                Protocol::AnthropicMessages,
                &json!({"content":[{"type":"thinking","thinking":"message thought"}]})
            ),
            "message thought"
        );
    }
}
