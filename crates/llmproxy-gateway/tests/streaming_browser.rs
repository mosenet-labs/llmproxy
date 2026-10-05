//! 手动浏览器验收夹具：独立数据库与模拟 Provider，不修改开发路由或调用真实模型。
mod nonstream;
#[allow(dead_code)]
mod streaming;
mod support;
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

#[tokio::test]
#[ignore = "手动浏览器验收：打印隔离页面 URL，等待最多 15 分钟；不调用真实模型"]
async fn browser_streaming_fixture() {
    let database = Database::new().await;
    let mut upstreams = Vec::new();
    for target in ALL {
        let (upstream, _) = Mock::http(move |request, socket| {
            sse_headers(socket);
            let body = String::from_utf8_lossy(&request.body);
            if body.contains("tool") {
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
