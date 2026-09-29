use std::time::{Duration, Instant};

use llmproxy_core::protocol::{MessagesAuth, Protocol};
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use serde_json::{Value, json};

/// Server-only configuration. The credential is deliberately excluded from Debug.
pub struct InferenceProbeTarget {
    pub url: Url,
    pub protocol: Protocol,
    pub secret: String,
    pub anthropic_version: Option<String>,
    pub messages_auth: MessagesAuth,
    pub timeout: Duration,
}

pub struct ModelProber {
    client: Client,
}

impl ModelProber {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder().redirect(Policy::none()).build()?,
        })
    }

    /// Tests one configured model and protocol with a one-message inference request.
    /// It does not persist the result or retain the generated content.
    pub async fn probe_model(
        &self,
        target: &InferenceProbeTarget,
        model_id: &str,
        max_output_tokens: u32,
    ) -> ProbeResult {
        run_probe(&self.client, target, model_id, max_output_tokens).await
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Available,
    Unavailable,
    Inconclusive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    ModelNotFound,
    Authentication,
    RateLimited,
    Timeout,
    Connection,
    InvalidRequest,
    UpstreamError,
    InvalidResponse,
    ThinkingStillEnabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingMode {
    DisabledRequested,
    Low,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ProbeResult {
    pub verdict: Verdict,
    pub reason: Option<Reason>,
    pub http_status: Option<u16>,
    pub usage: Option<TokenUsage>,
    pub elapsed: Duration,
    pub thinking_mode: ThinkingMode,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ThinkingControl {
    VendorDisabled,
    StandardDisabled,
    VendorLow,
    StandardLow,
}

impl ThinkingControl {
    fn mode(self) -> ThinkingMode {
        match self {
            Self::VendorDisabled | Self::StandardDisabled => ThinkingMode::DisabledRequested,
            Self::VendorLow | Self::StandardLow => ThinkingMode::Low,
        }
    }
}

async fn run_probe(
    client: &Client,
    target: &InferenceProbeTarget,
    model_id: &str,
    max_output_tokens: u32,
) -> ProbeResult {
    let start = Instant::now();
    let mut thinking = if target.protocol == Protocol::OpenAiChat
        && matches!(
            model_id.to_ascii_lowercase().as_str(),
            "glm-5.3" | "glm-5.3-flash"
        ) {
        ThinkingControl::VendorLow
    } else if target.protocol == Protocol::OpenAiResponses {
        ThinkingControl::StandardDisabled
    } else if target.protocol == Protocol::Gemini {
        ThinkingControl::StandardLow
    } else {
        ThinkingControl::VendorDisabled
    };
    if model_id.is_empty() || max_output_tokens == 0 {
        return result(
            Verdict::Inconclusive,
            Some(Reason::InvalidRequest),
            None,
            None,
            start,
            thinking.mode(),
        );
    }

    let mut chat_limit_field = "max_tokens";
    let (mut status, mut value) = match send_once(
        client,
        target,
        model_id,
        max_output_tokens,
        chat_limit_field,
        thinking,
        target.timeout,
    )
    .await
    {
        Ok(response) => response,
        Err((reason, status)) => {
            return result(
                Verdict::Inconclusive,
                Some(reason),
                status,
                None,
                start,
                thinking.mode(),
            );
        }
    };
    // Retry only after an explicit parameter rejection. Never retry without
    // a thinking control, which could spend unbounded reasoning tokens.
    for _ in 0..3 {
        if status != StatusCode::BAD_REQUEST {
            break;
        }
        if target.protocol == Protocol::OpenAiChat
            && chat_limit_field == "max_tokens"
            && unsupported_field(value.as_ref(), "max_tokens")
        {
            chat_limit_field = "max_completion_tokens";
        } else if thinking.mode() == ThinkingMode::DisabledRequested
            && cannot_disable_thinking(value.as_ref())
        {
            thinking = if thinking == ThinkingControl::StandardDisabled {
                ThinkingControl::StandardLow
            } else {
                ThinkingControl::VendorLow
            };
        } else if target.protocol == Protocol::OpenAiChat
            && thinking == ThinkingControl::VendorDisabled
            && unsupported_field(value.as_ref(), "thinking")
        {
            thinking = ThinkingControl::StandardDisabled;
        } else {
            break;
        }
        let remaining = target.timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return result(
                Verdict::Inconclusive,
                Some(Reason::Timeout),
                None,
                None,
                start,
                thinking.mode(),
            );
        }
        match send_once(
            client,
            target,
            model_id,
            max_output_tokens,
            chat_limit_field,
            thinking,
            remaining,
        )
        .await
        {
            Ok(response) => (status, value) = response,
            Err((reason, status)) => {
                return result(
                    Verdict::Inconclusive,
                    Some(reason),
                    status,
                    None,
                    start,
                    thinking.mode(),
                );
            }
        }
    }
    if !status.is_success() {
        let model_not_found = value.as_ref().is_some_and(|value| {
            let error = &value["error"];
            error["code"] == "model_not_found" || error["type"] == "model_not_found"
        });
        let (verdict, reason) = if model_not_found {
            (Verdict::Unavailable, Reason::ModelNotFound)
        } else {
            (
                Verdict::Inconclusive,
                match status {
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Reason::Authentication,
                    StatusCode::TOO_MANY_REQUESTS => Reason::RateLimited,
                    StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
                        Reason::InvalidRequest
                    }
                    _ => Reason::UpstreamError,
                },
            )
        };
        return result(
            verdict,
            Some(reason),
            Some(status.as_u16()),
            None,
            start,
            thinking.mode(),
        );
    }
    let Some(value) = value else {
        return result(
            Verdict::Inconclusive,
            Some(Reason::InvalidResponse),
            Some(status.as_u16()),
            None,
            start,
            thinking.mode(),
        );
    };
    let valid = match target.protocol {
        Protocol::OpenAiChat => value["choices"]
            .as_array()
            .is_some_and(|choices| !choices.is_empty()),
        Protocol::OpenAiResponses => {
            value["output"].is_array()
                && (value["status"] == "completed"
                    || (value["status"] == "incomplete"
                        && value["incomplete_details"]["reason"] == "max_output_tokens"))
        }
        Protocol::AnthropicMessages => value["type"] == "message" && value["content"].is_array(),
        Protocol::Gemini => value["candidates"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
    };
    if !valid {
        return result(
            Verdict::Inconclusive,
            Some(Reason::InvalidResponse),
            Some(status.as_u16()),
            None,
            start,
            thinking.mode(),
        );
    }
    let usage = match target.protocol {
        Protocol::OpenAiChat => token_usage(&value["usage"], "prompt_tokens", "completion_tokens"),
        Protocol::OpenAiResponses | Protocol::AnthropicMessages => {
            token_usage(&value["usage"], "input_tokens", "output_tokens")
        }
        Protocol::Gemini => token_usage(
            &value["usageMetadata"],
            "promptTokenCount",
            "candidatesTokenCount",
        ),
    };
    if thinking.mode() == ThinkingMode::DisabledRequested
        && response_contains_reasoning(target.protocol, &value)
    {
        return result(
            Verdict::Inconclusive,
            Some(Reason::ThinkingStillEnabled),
            Some(status.as_u16()),
            usage,
            start,
            thinking.mode(),
        );
    }
    result(
        Verdict::Available,
        None,
        Some(status.as_u16()),
        usage,
        start,
        thinking.mode(),
    )
}

async fn send_once(
    client: &Client,
    target: &InferenceProbeTarget,
    model_id: &str,
    max_output_tokens: u32,
    chat_limit_field: &str,
    thinking: ThinkingControl,
    timeout: Duration,
) -> Result<(StatusCode, Option<Value>), (Reason, Option<u16>)> {
    let mut body = match target.protocol {
        Protocol::OpenAiChat => json!({
            "model": model_id,
            "messages": [{"role": "user", "content": "你好"}],
            "stream": false,
        }),
        Protocol::OpenAiResponses => json!({
            "model": model_id,
            "input": "你好",
            "max_output_tokens": max_output_tokens,
            "stream": false,
        }),
        Protocol::AnthropicMessages => json!({
            "model": model_id,
            "messages": [{"role": "user", "content": "你好"}],
            "max_tokens": max_output_tokens,
            "stream": false,
        }),
        Protocol::Gemini => json!({
            "contents": [{"role": "user", "parts": [{"text": "你好"}]}],
            "generationConfig": {"maxOutputTokens": max_output_tokens},
        }),
    };
    if target.protocol == Protocol::OpenAiChat {
        body[chat_limit_field] = json!(max_output_tokens);
    }
    match (target.protocol, thinking) {
        (Protocol::OpenAiChat, ThinkingControl::VendorDisabled)
        | (Protocol::AnthropicMessages, ThinkingControl::VendorDisabled) => {
            body["thinking"] = json!({"type": "disabled"});
        }
        (Protocol::OpenAiChat, ThinkingControl::StandardDisabled) => {
            body["reasoning_effort"] = json!("none");
        }
        (Protocol::OpenAiResponses, ThinkingControl::StandardDisabled) => {
            body["reasoning"] = json!({"effort": "none"});
        }
        (Protocol::OpenAiChat, ThinkingControl::VendorLow) => {
            body["thinking"] = json!({"type": "enabled"});
            body["reasoning_effort"] = json!("low");
        }
        (Protocol::OpenAiChat, ThinkingControl::StandardLow) => {
            body["reasoning_effort"] = json!("low");
        }
        (Protocol::OpenAiResponses, ThinkingControl::StandardLow) => {
            body["reasoning"] = json!({"effort": "low"});
        }
        (Protocol::AnthropicMessages, ThinkingControl::VendorLow) => {
            body["thinking"] = json!({"type": "adaptive"});
            body["output_config"] = json!({"effort": "low"});
        }
        (Protocol::OpenAiResponses, ThinkingControl::VendorDisabled)
        | (Protocol::OpenAiResponses, ThinkingControl::VendorLow)
        | (Protocol::AnthropicMessages, ThinkingControl::StandardDisabled)
        | (Protocol::AnthropicMessages, ThinkingControl::StandardLow) => unreachable!(),
        (Protocol::Gemini, ThinkingControl::StandardLow) => {}
        (Protocol::Gemini, _) => unreachable!(),
    }
    let mut url = target.url.clone();
    if target.protocol == Protocol::Gemini {
        let model = model_id.strip_prefix("models/").unwrap_or(model_id);
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| (Reason::InvalidRequest, None))?;
        segments.push(&format!("{model}:generateContent"));
    }
    let request = client
        .post(url)
        .timeout(timeout)
        .header("accept", "application/json")
        .json(&body);
    let request = match target.protocol {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => request.bearer_auth(&target.secret),
        Protocol::AnthropicMessages => {
            let request = match target.messages_auth {
                MessagesAuth::ApiKey => request.header("x-api-key", &target.secret),
                MessagesAuth::Bearer => request.bearer_auth(&target.secret),
            };
            request.header(
                "anthropic-version",
                target.anthropic_version.as_deref().unwrap_or("2023-06-01"),
            )
        }
        Protocol::Gemini => request.header("x-goog-api-key", &target.secret),
    };
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let reason = if error.is_timeout() {
                Reason::Timeout
            } else {
                Reason::Connection
            };
            return Err((reason, None));
        }
    };
    let status = response.status();
    let mut bytes = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                return Err((
                    if error.is_timeout() {
                        Reason::Timeout
                    } else {
                        Reason::Connection
                    },
                    Some(status.as_u16()),
                ));
            }
        };
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            return Err((Reason::InvalidResponse, Some(status.as_u16())));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, serde_json::from_slice::<Value>(&bytes).ok()))
}

fn unsupported_field(value: Option<&Value>, field: &str) -> bool {
    let Some(value) = value else { return false };
    let error = &value["error"];
    if error["code"] == "unsupported_parameter" && error["param"] == field {
        return true;
    }
    error["message"].as_str().is_some_and(|message| {
        let message = message.to_ascii_lowercase();
        message.contains(field)
            && ["unsupported", "not supported", "unknown", "unrecognized"]
                .iter()
                .any(|marker| message.contains(marker))
    })
}

fn cannot_disable_thinking(value: Option<&Value>) -> bool {
    let Some(message) = value
        .and_then(|value| value["error"]["message"].as_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    (message.contains("thinking") || message.contains("reasoning"))
        && (message.contains("disabled") || message.contains("none"))
        && [
            "not supported",
            "cannot",
            "only supports",
            "supported values",
            "invalid value",
        ]
        .iter()
        .any(|marker| message.contains(marker))
}

fn response_contains_reasoning(protocol: Protocol, value: &Value) -> bool {
    let details = match protocol {
        Protocol::OpenAiChat => &value["usage"]["completion_tokens_details"],
        Protocol::OpenAiResponses => &value["usage"]["output_tokens_details"],
        Protocol::AnthropicMessages => &value["usage"],
        Protocol::Gemini => &value["usageMetadata"],
    };
    if details["reasoning_tokens"]
        .as_u64()
        .is_some_and(|count| count > 0)
    {
        return true;
    }
    match protocol {
        Protocol::OpenAiChat => value["choices"].as_array().is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice["message"]["reasoning_content"]
                    .as_str()
                    .is_some_and(|content| !content.is_empty())
            })
        }),
        Protocol::OpenAiResponses => value["output"]
            .as_array()
            .is_some_and(|output| output.iter().any(|item| item["type"] == "reasoning")),
        Protocol::AnthropicMessages => value["content"]
            .as_array()
            .is_some_and(|content| content.iter().any(|item| item["type"] == "thinking")),
        Protocol::Gemini => false,
    }
}

fn token_usage(value: &Value, input_key: &str, output_key: &str) -> Option<TokenUsage> {
    Some(TokenUsage {
        input: value.get(input_key)?.as_u64()?,
        output: value.get(output_key)?.as_u64()?,
    })
}

fn result(
    verdict: Verdict,
    reason: Option<Reason>,
    http_status: Option<u16>,
    usage: Option<TokenUsage>,
    start: Instant,
    thinking_mode: ThinkingMode,
) -> ProbeResult {
    ProbeResult {
        verdict,
        reason,
        http_status,
        usage,
        elapsed: start.elapsed(),
        thinking_mode,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    use super::*;

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut received = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&chunk[..count]);
            if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&received[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap();
                if received.len() >= end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8(received).unwrap()
    }

    fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    }

    async fn fake_upstream(protocol: Protocol, status: u16, body: &str) -> (ProbeResult, String) {
        fake_upstream_model(protocol, status, body, "test-model").await
    }

    async fn fake_upstream_model(
        protocol: Protocol,
        status: u16,
        body: &str,
        model_id: &str,
    ) -> (ProbeResult, String) {
        fake_upstream_model_auth(protocol, status, body, model_id, MessagesAuth::ApiKey).await
    }

    async fn fake_upstream_model_auth(
        protocol: Protocol,
        status: u16,
        body: &str,
        model_id: &str,
        messages_auth: MessagesAuth,
    ) -> (ProbeResult, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = body.to_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            write_response(&mut stream, status, &reply);
            request
        });
        let target = InferenceProbeTarget {
            url: format!("http://{address}/infer").parse().unwrap(),
            protocol,
            secret: "test-secret".into(),
            anthropic_version: Some("2023-06-01".into()),
            messages_auth,
            timeout: Duration::from_secs(3),
        };
        let result = ModelProber::new()
            .unwrap()
            .probe_model(&target, model_id, 1)
            .await;
        (result, server.join().unwrap())
    }

    #[tokio::test]
    async fn sends_minimal_request_for_each_protocol() {
        let cases = [
            (
                Protocol::OpenAiChat,
                r#"{"choices":[{"finish_reason":"length"}],"usage":{"prompt_tokens":8,"completion_tokens":1}}"#,
                "max_tokens",
                "authorization: Bearer test-secret",
            ),
            (
                Protocol::OpenAiResponses,
                r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[],"usage":{"input_tokens":8,"output_tokens":1}}"#,
                "max_output_tokens",
                "authorization: Bearer test-secret",
            ),
            (
                Protocol::AnthropicMessages,
                r#"{"type":"message","content":[],"usage":{"input_tokens":8,"output_tokens":1}}"#,
                "max_tokens",
                "x-api-key: test-secret",
            ),
        ];
        for (protocol, body, limit_key, auth) in cases {
            let (result, request) = fake_upstream(protocol, 200, body).await;
            assert_eq!(result.verdict, Verdict::Available);
            assert_eq!(
                result.usage,
                Some(TokenUsage {
                    input: 8,
                    output: 1
                })
            );
            assert!(request.contains("POST /infer HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains(&auth.to_ascii_lowercase())
            );
            let payload: Value =
                serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(payload["model"], "test-model");
            assert_eq!(payload[limit_key], 1);
            assert_eq!(payload["stream"], false);
            match protocol {
                Protocol::OpenAiResponses => {
                    assert_eq!(payload["input"], "你好");
                    assert_eq!(payload["reasoning"]["effort"], "none");
                }
                Protocol::OpenAiChat | Protocol::AnthropicMessages => {
                    assert_eq!(payload["messages"][0]["content"], "你好");
                    assert_eq!(payload["thinking"]["type"], "disabled");
                }
                Protocol::Gemini => unreachable!(),
            }
            if protocol == Protocol::AnthropicMessages {
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("anthropic-version: 2023-06-01")
                );
            }
        }
    }

    #[tokio::test]
    async fn probes_gemini_native_model_path_and_response() {
        let (result, request) = fake_upstream(
            Protocol::Gemini,
            200,
            r#"{"candidates":[{"content":{"parts":[{"text":"你好"}]}}],"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":1}}"#,
        ).await;
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(
            result.usage,
            Some(TokenUsage {
                input: 8,
                output: 1
            })
        );
        assert!(request.contains("POST /infer/test-model:generateContent HTTP/1.1"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-goog-api-key: test-secret")
        );
        let payload: Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(payload["contents"][0]["parts"][0]["text"], "你好");
        assert_eq!(payload["generationConfig"]["maxOutputTokens"], 1);
    }

    #[tokio::test]
    async fn messages_probe_uses_provider_bearer_auth() {
        let (_, request) = fake_upstream_model_auth(
            Protocol::AnthropicMessages,
            200,
            r#"{"type":"message","content":[],"usage":{"input_tokens":8,"output_tokens":1}}"#,
            "test-model",
            MessagesAuth::Bearer,
        )
        .await;
        let request = request.to_ascii_lowercase();
        assert!(request.contains("authorization: bearer test-secret"));
        assert!(!request.contains("x-api-key:"));
    }

    #[tokio::test]
    async fn separates_model_not_found_from_upstream_failures() {
        let (missing, _) = fake_upstream(
            Protocol::OpenAiChat,
            404,
            r#"{"error":{"code":"model_not_found"}}"#,
        )
        .await;
        assert_eq!(missing.verdict, Verdict::Unavailable);
        assert_eq!(missing.reason, Some(Reason::ModelNotFound));

        let (route_error, _) = fake_upstream(
            Protocol::OpenAiChat,
            404,
            r#"{"error":{"code":"not_found"}}"#,
        )
        .await;
        assert_eq!(route_error.verdict, Verdict::Inconclusive);

        let (limited, _) = fake_upstream(
            Protocol::OpenAiChat,
            429,
            r#"{"error":{"code":"rate_limit"}}"#,
        )
        .await;
        assert_eq!(limited.verdict, Verdict::Inconclusive);
        assert_eq!(limited.reason, Some(Reason::RateLimited));
    }

    #[tokio::test]
    async fn retries_chat_with_new_limit_field_only_when_old_field_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let original = read_request(&mut first);
            write_response(
                &mut first,
                400,
                r#"{"error":{"code":"unsupported_parameter","param":"max_tokens"}}"#,
            );
            let (mut second, _) = listener.accept().unwrap();
            let retry = read_request(&mut second);
            write_response(
                &mut second,
                200,
                r#"{"choices":[{"finish_reason":"length"}]}"#,
            );
            (original, retry)
        });
        let target = InferenceProbeTarget {
            url: format!("http://{address}/infer").parse().unwrap(),
            protocol: Protocol::OpenAiChat,
            secret: "test-secret".into(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            timeout: Duration::from_secs(3),
        };
        let result = ModelProber::new()
            .unwrap()
            .probe_model(&target, "test-model", 1)
            .await;
        assert_eq!(result.verdict, Verdict::Available);
        let (original, retry) = server.join().unwrap();
        let original: Value =
            serde_json::from_str(original.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let retry: Value = serde_json::from_str(retry.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(original["max_tokens"], 1);
        assert!(original.get("max_completion_tokens").is_none());
        assert_eq!(retry["max_completion_tokens"], 1);
        assert!(retry.get("max_tokens").is_none());
        assert_eq!(retry["thinking"]["type"], "disabled");
    }

    #[tokio::test]
    async fn retries_chat_with_standard_control_when_thinking_field_is_unknown() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let original = read_request(&mut first);
            write_response(
                &mut first,
                400,
                r#"{"error":{"message":"Unknown parameter: thinking"}}"#,
            );
            let (mut second, _) = listener.accept().unwrap();
            let retry = read_request(&mut second);
            write_response(
                &mut second,
                200,
                r#"{"choices":[{"finish_reason":"length"}]}"#,
            );
            (original, retry)
        });
        let target = InferenceProbeTarget {
            url: format!("http://{address}/infer").parse().unwrap(),
            protocol: Protocol::OpenAiChat,
            secret: "test-secret".into(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            timeout: Duration::from_secs(3),
        };
        let result = ModelProber::new()
            .unwrap()
            .probe_model(&target, "test-model", 1)
            .await;
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(result.thinking_mode, ThinkingMode::DisabledRequested);
        let (original, retry) = server.join().unwrap();
        let original: Value =
            serde_json::from_str(original.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let retry: Value = serde_json::from_str(retry.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(original["thinking"]["type"], "disabled");
        assert_eq!(retry["reasoning_effort"], "none");
        assert!(retry.get("thinking").is_none());
    }

    #[tokio::test]
    async fn forced_thinking_glm_uses_low_from_the_first_request() {
        let (result, request) = fake_upstream_model(
            Protocol::OpenAiChat,
            200,
            r#"{"choices":[{"finish_reason":"length"}],"usage":{"prompt_tokens":8,"completion_tokens":1}}"#,
            "glm-5.3-flash",
        )
        .await;
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(result.thinking_mode, ThinkingMode::Low);
        let payload: Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(payload["thinking"]["type"], "enabled");
        assert_eq!(payload["reasoning_effort"], "low");
        assert_eq!(payload["max_tokens"], 1);
    }

    #[tokio::test]
    async fn standard_chat_low_fallback_does_not_send_vendor_thinking_field() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            read_request(&mut first);
            write_response(
                &mut first,
                400,
                r#"{"error":{"message":"Unknown parameter: thinking"}}"#,
            );
            let (mut second, _) = listener.accept().unwrap();
            read_request(&mut second);
            write_response(
                &mut second,
                400,
                r#"{"error":{"message":"reasoning_effort none is not supported for this model"}}"#,
            );
            let (mut third, _) = listener.accept().unwrap();
            let retry = read_request(&mut third);
            write_response(
                &mut third,
                200,
                r#"{"choices":[{"finish_reason":"length"}]}"#,
            );
            retry
        });
        let target = InferenceProbeTarget {
            url: format!("http://{address}/infer").parse().unwrap(),
            protocol: Protocol::OpenAiChat,
            secret: "test-secret".into(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            timeout: Duration::from_secs(3),
        };
        let result = ModelProber::new()
            .unwrap()
            .probe_model(&target, "test-model", 1)
            .await;
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(result.thinking_mode, ThinkingMode::Low);
        let retry = server.join().unwrap();
        let retry: Value = serde_json::from_str(retry.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(retry["reasoning_effort"], "low");
        assert!(retry.get("thinking").is_none());
    }

    #[tokio::test]
    async fn retries_messages_with_low_when_thinking_cannot_be_disabled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let original = read_request(&mut first);
            write_response(
                &mut first,
                400,
                r#"{"error":{"message":"thinking.type.disabled is not supported for this model"}}"#,
            );
            let (mut second, _) = listener.accept().unwrap();
            let retry = read_request(&mut second);
            write_response(&mut second, 200, r#"{"type":"message","content":[]}"#);
            (original, retry)
        });
        let target = InferenceProbeTarget {
            url: format!("http://{address}/infer").parse().unwrap(),
            protocol: Protocol::AnthropicMessages,
            secret: "test-secret".into(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            timeout: Duration::from_secs(3),
        };
        let result = ModelProber::new()
            .unwrap()
            .probe_model(&target, "test-model", 1)
            .await;
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(result.thinking_mode, ThinkingMode::Low);
        let (original, retry) = server.join().unwrap();
        let original: Value =
            serde_json::from_str(original.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let retry: Value = serde_json::from_str(retry.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(original["thinking"]["type"], "disabled");
        assert_eq!(retry["thinking"]["type"], "adaptive");
        assert_eq!(retry["output_config"]["effort"], "low");
    }

    #[tokio::test]
    async fn retries_responses_with_low_when_none_is_not_supported() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let original = read_request(&mut first);
            write_response(
                &mut first,
                400,
                r#"{"error":{"message":"reasoning.effort none is not supported for this model"}}"#,
            );
            let (mut second, _) = listener.accept().unwrap();
            let retry = read_request(&mut second);
            write_response(&mut second, 200, r#"{"status":"completed","output":[]}"#);
            (original, retry)
        });
        let target = InferenceProbeTarget {
            url: format!("http://{address}/infer").parse().unwrap(),
            protocol: Protocol::OpenAiResponses,
            secret: "test-secret".into(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            timeout: Duration::from_secs(3),
        };
        let result = ModelProber::new()
            .unwrap()
            .probe_model(&target, "test-model", 1)
            .await;
        assert_eq!(result.verdict, Verdict::Available);
        assert_eq!(result.thinking_mode, ThinkingMode::Low);
        let (original, retry) = server.join().unwrap();
        let original: Value =
            serde_json::from_str(original.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let retry: Value = serde_json::from_str(retry.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(original["reasoning"]["effort"], "none");
        assert_eq!(retry["reasoning"]["effort"], "low");
    }

    #[tokio::test]
    async fn accepted_disable_control_with_reasoning_is_inconclusive() {
        let (result, _) = fake_upstream(
            Protocol::OpenAiChat,
            200,
            r#"{"choices":[{"message":{"reasoning_content":"thought"}}]}"#,
        )
        .await;
        assert_eq!(result.verdict, Verdict::Inconclusive);
        assert_eq!(result.reason, Some(Reason::ThinkingStillEnabled));
    }
}
