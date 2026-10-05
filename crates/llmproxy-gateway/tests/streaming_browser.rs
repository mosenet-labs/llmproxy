//! 手动浏览器验收夹具：独立数据库与模拟 Provider，不修改开发路由或调用真实模型。
mod nonstream;
#[allow(dead_code)]
mod streaming;
mod support;
use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::protocol::Protocol;
use nonstream::{ALL, Database, MASTER_KEY};
use support::{Gateway, Mock, chunk, finish_chunks, sse_headers};

/// 纯工具页面不制造普通文本；只从独立 SSE 夹具删除可见文本事件及最终文本快照。
fn tool_frames(protocol: Protocol) -> Vec<Vec<u8>> {
    let (start, end) = streaming::frames(protocol, true);
    start
        .into_iter()
        .chain(end)
        .filter_map(|frame| {
            let mut framing = llmproxy_core::protocol::stream::sse::Decoder::new(8 * 1024 * 1024);
            let raw = frame
                .iter()
                .find_map(|byte| framing.push(*byte).unwrap())
                .unwrap();
            if raw.data == b"[DONE]" {
                return Some(frame);
            }
            let mut value: serde_json::Value = serde_json::from_slice(&raw.data).unwrap();
            match protocol {
                Protocol::OpenAiChat => {
                    for choice in value["choices"].as_array_mut().unwrap() {
                        choice["delta"].as_object_mut().unwrap().remove("content");
                    }
                }
                Protocol::AnthropicMessages => {
                    if value["index"] == 0 {
                        return None;
                    }
                }
                Protocol::OpenAiResponses => {
                    if value["output_index"] == 0 {
                        return None;
                    }
                    if let Some(index) = value.get_mut("output_index") {
                        *index = serde_json::json!(0);
                    }
                    if let Some(output) = value
                        .get_mut("response")
                        .and_then(|response| response.get_mut("output"))
                        .and_then(serde_json::Value::as_array_mut)
                    {
                        output.retain(|item| item["type"] == "function_call");
                    }
                }
                Protocol::Gemini => {
                    if let Some(candidates) = value["candidates"].as_array_mut() {
                        for candidate in candidates {
                            if let Some(parts) = candidate
                                .get_mut("content")
                                .and_then(|content| content.get_mut("parts"))
                                .and_then(serde_json::Value::as_array_mut)
                            {
                                parts.retain(|part| part.get("text").is_none());
                                if parts.is_empty() {
                                    return None;
                                }
                            }
                        }
                    }
                }
            }
            Some(
                format!(
                    "{}data: {}\n\n",
                    raw.event
                        .map(|kind| format!("event: {kind}\n"))
                        .unwrap_or_default(),
                    value
                )
                .into_bytes(),
            )
        })
        .collect()
}

#[test]
fn pure_tool_browser_fixtures_are_valid_in_all_four_protocols() {
    for protocol in ALL {
        let mut observed = streaming::Observed::new(protocol);
        for frame in tool_frames(protocol) {
            observed.push(protocol, &frame);
        }
        observed.finish(protocol);
        assert!(observed.text.is_empty());
        assert_eq!(observed.tools.len(), 1, "{protocol:?}");
    }
}

/// 生成短 WAV 音调，浏览器必须实际解码；不依赖外部音频文件或真实模型。
fn wav() -> Vec<u8> {
    let size = 1600u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    for index in 0..800 {
        let sample =
            ((index as f64 * 440.0 * std::f64::consts::TAU / 8000.0).sin() * 3000.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// 两段分别编码且带 padding 的音频，转录在完整响应结束前交付。
/// 参考：https://developers.openai.com/api/reference/resources/responses/streaming-events
fn audio_frames(protocol: Protocol, bytes: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let (first, second) = bytes.split_at(bytes.len().min(31));
    let first = STANDARD.encode(first);
    let second = STANDARD.encode(second);
    let usage = serde_json::json!({"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}});
    let (start, end) = if protocol == Protocol::OpenAiChat {
        let chunk = |delta: serde_json::Value, finish: serde_json::Value| serde_json::json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        (
            vec![
                chunk(
                    serde_json::json!({"role":"assistant","audio":{"id":"private-audio-id","data":first,"transcript":"你好，"}}),
                    serde_json::Value::Null,
                ),
                chunk(
                    serde_json::json!({"audio":{"data":second,"transcript":"音频测试。"}}),
                    serde_json::Value::Null,
                ),
            ],
            vec![
                chunk(serde_json::json!({}), serde_json::json!("stop")),
                chunk(
                    serde_json::json!({"audio":{"expires_at":123}}),
                    serde_json::Value::Null,
                ),
                serde_json::json!({"id":"c","model":"m","created":1,"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":4}}}),
            ],
        )
    } else {
        assert_eq!(protocol, Protocol::OpenAiResponses);
        (
            vec![
                serde_json::json!({"type":"response.created","sequence_number":0,"response":{"id":"r","model":"m","created_at":1,"object":"response","output":[]}}),
                serde_json::json!({"type":"response.audio.transcript.delta","sequence_number":1,"delta":"你好，"}),
                serde_json::json!({"type":"response.audio.delta","sequence_number":2,"delta":first}),
                serde_json::json!({"type":"response.audio.delta","sequence_number":3,"delta":second}),
                serde_json::json!({"type":"response.audio.transcript.delta","sequence_number":4,"delta":"音频测试。"}),
            ],
            vec![
                serde_json::json!({"type":"response.audio.transcript.done","sequence_number":5}),
                serde_json::json!({"type":"response.audio.done","sequence_number":6}),
                serde_json::json!({"type":"response.completed","sequence_number":7,"response":{"id":"r","model":"m","created_at":1,"object":"response","status":"completed","output":[],"usage":usage}}),
            ],
        )
    };
    let frame = |value: serde_json::Value| {
        format!(
            "{}data: {value}\n\n",
            value["type"]
                .as_str()
                .map(|kind| format!("event: {kind}\n"))
                .unwrap_or_default()
        )
        .into_bytes()
    };
    let mut end: Vec<_> = end.into_iter().map(frame).collect();
    if protocol == Protocol::OpenAiChat {
        end.push(b"data: [DONE]\n\n".to_vec());
    }
    (start.into_iter().map(frame).collect(), end)
}

#[test]
fn native_audio_browser_fixtures_have_valid_independent_terminal_events() {
    for protocol in [Protocol::OpenAiChat, Protocol::OpenAiResponses] {
        let mut observed = streaming::Observed::new(protocol);
        let (start, end) = audio_frames(protocol, &wav());
        for frame in start.into_iter().chain(end) {
            observed.push(protocol, &frame);
        }
        observed.finish(protocol);
        assert!(observed.text.is_empty());
        assert_eq!(
            observed
                .decoder
                .state()
                .usage()
                .snapshot()
                .unwrap()
                .total_tokens,
            Some(13)
        );
    }
}

#[tokio::test]
#[ignore = "手动浏览器验收：打印隔离页面 URL，等待最多 15 分钟；不调用真实模型"]
async fn browser_streaming_fixture() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (upstream, _) = Mock::http(move |request, socket| {
            sse_headers(socket);
            let body = String::from_utf8_lossy(&request.body);
            if body.contains("audio")
                && matches!(target, Protocol::OpenAiChat | Protocol::OpenAiResponses)
            {
                let bytes = if body.contains("raw-audio") {
                    vec![1, 2, 3, 4]
                } else {
                    wav()
                };
                let (start, end) = audio_frames(target, &bytes);
                for frame in start {
                    if chunk(socket, &frame).is_err() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                if body.contains("error-audio") {
                    let _ = chunk(socket, b"data: private-provider-detail\n\n");
                    finish_chunks(socket);
                    return;
                }
                if body.contains("truncated-audio") {
                    finish_chunks(socket);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(
                    if body.contains("slow-audio") { 30 } else { 2 },
                ));
                for frame in end {
                    if chunk(socket, &frame).is_err() {
                        return;
                    }
                }
            } else if body.contains("tool") {
                for frame in tool_frames(target) {
                    if chunk(socket, &frame).is_err() {
                        return;
                    }
                }
            } else {
                let (start, end) = streaming::frames(target, false);
                for frame in start {
                    if chunk(socket, &frame).is_err() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                if body.contains("error") {
                    let _ = chunk(socket, b"data: private-provider-detail\n\n");
                    finish_chunks(socket);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(if body.contains("slow") {
                    30
                } else {
                    2
                }));
                for frame in end {
                    if chunk(socket, &frame).is_err() {
                        return;
                    }
                }
            }
            finish_chunks(socket);
        });
        database
            .add_provider(
                streaming::provider(target, upstream.address.port(), 60000),
                target,
                "upstream-model",
            )
            .await;
        upstreams.push(upstream);
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    println!("BROWSER http://{}/ui/chat", gateway.address);
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = tokio::time::sleep(std::time::Duration::from_secs(900)) => {},
    }
    drop(gateway);
    drop(upstreams);
}
