//! 控制台聊天的 HTTP 边界：非流式读取整体响应 IR，流式逐事件读取并汇总历史。
mod audio;
mod completed;
mod display;
pub(crate) mod history;
mod incremental;
#[cfg(test)]
mod incremental_tests;
mod media;
mod request;
pub(crate) use display::DisplayPart;
use history::{Conversation, Selection};
use llmproxy_core::protocol::Protocol;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use request::{request_body, with_request_body};
use reqwest::Client;
use serde_json::Value;

const MAX_REPLY_BYTES: usize = 256 * 1024;
const MODEL_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub(crate) use llmproxy_core::conversation::Reply as ChatReply;

fn error_message(value: &Value) -> String {
    value
        .as_str()
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("上游请求失败")
        .to_owned()
}

// HTTP 边界分别接收当前轮历史、模式和更新回调。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn chat_reply(
    client: &Client,
    gateway_origin: &str,
    selection: &Selection,
    alias: &str,
    history: &Conversation,
    stream: bool,
    thinking: llmproxy_core::thinking::Choice,
    history_request: Option<(&str, &str)>,
    mut on_update: impl FnMut(&ChatReply),
) -> Result<ChatReply, String> {
    let protocol = selection.protocol;
    let converted = request_body(selection, alias, history, stream)?;
    for warning in &converted.warnings {
        tracing::warn!(
            component = "console",
            event_kind = "chat_history_conversion",
            source_protocol = warning.source.as_str(),
            target_protocol = warning.target.as_str(),
            path = warning.path.as_str(),
            reason = warning.reason.as_str(),
            "chat history conversion warning"
        );
    }
    let mut warnings: Vec<_> = converted
        .warnings
        .iter()
        .map(|warning| warning.reason.clone())
        .collect();
    warnings.sort();
    warnings.dedup();
    if !warnings.is_empty() {
        // 即使后续 HTTP 失败，已作出的历史兼容处理也应在这一轮可见。
        on_update(&ChatReply {
            warnings: warnings.clone(),
            ..Default::default()
        });
    }
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
    let builder = if let Some((key, auth)) = history_request {
        builder
            .header(llmproxy_store::chat_history::REQUEST_HEADER, key)
            .header(llmproxy_store::chat_history::AUTH_HEADER, auth)
    } else {
        builder
    };
    let response = with_request_body(
        builder.header(llmproxy_core::thinking::HEADER, thinking.as_str()),
        &converted.body,
    )
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
        let mut reply = completed::decode(protocol, &bytes)?;
        reply.warnings = warnings;
        on_update(&reply);
        return Ok(reply);
    }
    if !stream {
        return Err("上游未返回非流式 JSON 响应".into());
    }
    let mut response = response;
    let mut reply = incremental::Reply::new(protocol);
    reply.set_warnings(warnings);
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取上游响应失败")? {
        reply.push(&chunk, &mut on_update)?;
    }
    let reply = reply.finish()?;
    on_update(&reply);
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::history::{Content, Conversation, Selection};
    use super::{request_body, with_request_body};
    use llmproxy_core::ir::message::Role;
    use llmproxy_core::protocol::Protocol;
    use serde_json::json;

    #[test]
    fn request_bodies_match_each_protocol() {
        let mut history = Conversation::default();
        let origin = Selection {
            model_id: "1".into(),
            protocol: Protocol::OpenAiChat,
        };
        history.append(&origin, Content::text(Role::User, "hi"));
        history.append(&origin, Content::text(Role::Assistant, "你好"));
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
            let selection = Selection {
                model_id: "1".into(),
                protocol,
            };
            let body = request_body(&selection, "alias", &history, true)
                .unwrap()
                .body;
            assert_eq!(body.protocol(), protocol);
            // 检查实际 HTTP 请求正文，确保序列化时没有额外的枚举标签或默认字段。
            let request = with_request_body(client.post("http://localhost/test"), &body)
                .build()
                .unwrap();
            assert_eq!(request.headers()["content-type"], "application/json");
            let actual: serde_json::Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            assert_eq!(actual, expected, "{protocol:?}");
            let body = request_body(&selection, "alias", &history, false)
                .unwrap()
                .body;
            let request = with_request_body(client.post("http://localhost/test"), &body)
                .build()
                .unwrap();
            let actual: serde_json::Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            if protocol == Protocol::Gemini {
                assert!(actual.get("stream").is_none());
            } else {
                assert!(!actual["stream"].as_bool().unwrap_or_default());
            }
        }
    }
}
