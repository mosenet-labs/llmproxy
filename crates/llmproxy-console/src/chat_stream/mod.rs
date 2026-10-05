//! 控制台聊天的 HTTP 边界：非流式读取整包，已有同协议流式路径逐事件读取。
mod audio;
mod completed;
mod display;
mod incremental;
#[cfg(test)]
mod incremental_tests;
mod media;
mod request;
pub(crate) use display::DisplayPart;
use llmproxy_core::protocol::Protocol;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use request::{request_body, with_request_body};
use reqwest::Client;
use serde_json::Value;
use topcoat_ant_design::ChatMessage;

const MAX_REPLY_BYTES: usize = 256 * 1024;
const MODEL_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

#[derive(Debug, Default)]
pub(crate) struct ChatReply {
    /// 显示给用户的回答正文。
    pub content: String,
    /// 可见思考内容。
    pub thinking: String,
    /// 可见思考的摘要。
    pub summary: String,
    /// 经 IR 统一口径的本轮统计，缺失计数不补零。
    pub usage: Option<llmproxy_core::ir::usage::Usage>,
    /// 工具、媒体和服务端执行内容；展示标签不进入下一轮文本历史。
    pub parts: Vec<DisplayPart>,
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

fn error_message(value: &Value) -> String {
    value
        .as_str()
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("上游请求失败")
        .to_owned()
}

pub async fn chat_reply(
    client: &Client,
    gateway_origin: &str,
    protocol: Protocol,
    alias: &str,
    history: &[ChatMessage],
    stream: bool,
    mut on_update: impl FnMut(&ChatReply),
) -> Result<ChatReply, String> {
    let path = if protocol == Protocol::Gemini {
        let model = utf8_percent_encode(alias, MODEL_SEGMENT);
        let method = if stream {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };
        format!("/v1beta/models/{model}:{method}")
    } else {
        protocol.upstream_path().to_owned()
    };
    let builder = client.post(format!("{gateway_origin}{path}")).header(
        "accept",
        if stream {
            "text/event-stream"
        } else {
            "application/json"
        },
    );
    let response = with_request_body(builder, &request_body(protocol, alias, history, stream))
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
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "读取上游响应失败")? {
            if bytes.len().saturating_add(chunk.len()) > MAX_REPLY_BYTES {
                return Err("上游响应过大".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let reply = completed::decode(protocol, &bytes)?;
        on_update(&reply);
        return Ok(reply);
    }
    if !stream {
        return Err("上游未返回非流式 JSON 响应".into());
    }
    let mut response = response;
    let mut reply = incremental::Reply::new(protocol);
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取上游响应失败")? {
        reply.push(&chunk, &mut on_update)?;
    }
    let reply = reply.finish()?;
    on_update(&reply);
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::{request_body, with_request_body};
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
                ChatMessageStatus::Complete,
                "你好",
            ),
            ChatMessage::new(
                "3",
                ChatBubbleRole::Assistant,
                ChatMessageStatus::Sending,
                "未完成的回复",
            ),
        ];
        let messages = json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "你好"},
        ]);
        let client = reqwest::Client::new();
        for (protocol, expected) in [
            (
                Protocol::OpenAiChat,
                json!({
                    "model": "alias", "stream": true, "max_completion_tokens":2048, "messages": messages, "stream_options":{"include_usage":true},
                }),
            ),
            (
                Protocol::OpenAiResponses,
                json!({
                    "model": "alias", "stream": true, "max_output_tokens":2048, "input": messages,
                }),
            ),
            (
                Protocol::AnthropicMessages,
                json!({
                    "model": "alias", "stream": true, "max_tokens": 2048, "messages": messages,
                }),
            ),
            (
                Protocol::Gemini,
                json!({"generationConfig":{"maxOutputTokens":2048},"contents": [
                    {"role": "user", "parts": [{"text": "hi"}]},
                    {"role": "model", "parts": [{"text": "你好"}]},
                ]}),
            ),
        ] {
            let body = request_body(protocol, "alias", &history, true);
            assert_eq!(body.protocol(), protocol);
            // 检查实际 HTTP 请求正文，确保序列化时没有额外的枚举标签或默认字段。
            let request = with_request_body(client.post("http://localhost/test"), &body)
                .build()
                .unwrap();
            assert_eq!(request.headers()["content-type"], "application/json");
            let actual: serde_json::Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            assert_eq!(actual, expected, "{protocol:?}");
            let body = request_body(protocol, "alias", &history, false);
            let request = with_request_body(client.post("http://localhost/test"), &body)
                .build()
                .unwrap();
            let actual: serde_json::Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            if protocol == Protocol::Gemini {
                assert!(actual.get("stream").is_none());
            } else {
                assert_eq!(actual["stream"], false);
            }
        }
    }
}
