mod nonstream;
#[allow(dead_code)]
mod streaming;
mod support;
use llmproxy_core::protocol::Protocol;
use nonstream::{Database, MASTER_KEY, alias, fixtures};
use std::sync::mpsc;
use support::{Gateway, Mock, chunk, sse_headers};

#[tokio::test]
async fn oversized_event_fails_without_a_success_terminal() {
    let (upstream, _) = Mock::http(|_, socket| {
        sse_headers(socket);
        for frame in streaming::frames(Protocol::OpenAiChat, false).0 {
            chunk(socket, &frame).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        // 注释也占事件预算，不能用忽略字段绕过 8 MiB 单帧限制。
        let mut oversized = vec![b'x'; 9 * 1024 * 1024];
        oversized[0] = b':';
        let _ = chunk(socket, &oversized);
    });
    let database = Database::new().await;
    database
        .add_provider(
            streaming::provider(Protocol::OpenAiChat, upstream.address.port(), 3000),
            Protocol::OpenAiChat,
            "upstream-model",
        )
        .await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let alias = alias(Protocol::AnthropicMessages, Protocol::OpenAiChat);
    let mut input = fixtures::request(Protocol::AnthropicMessages, &alias, "capacity");
    input["stream"] = serde_json::json!(true);
    let response = gateway.request(
        "POST",
        "/v1/messages",
        "Content-Type: application/json\r\n",
        &serde_json::to_vec(&input).unwrap(),
    );
    assert_eq!(response.status, 200);
    let bytes = response.body();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("你") && text.contains("好"));
    assert!(text.contains("event: error"));
    assert!(!text.contains("message_stop"));
    streaming::assert_logs(&gateway, 1);
}

#[tokio::test]
async fn slow_client_applies_backpressure_and_write_timeout() {
    let (closed, receive) = mpsc::channel();
    let (upstream, _) = Mock::http(move |_, socket| {
        socket
            .set_write_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        sse_headers(socket);
        let frame = format!(
            "data: {}\n\n",
            serde_json::json!({"id":"raw","model":"m","created":1,"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"x".repeat(8192)}}]})
        );
        let mut sent = 0;
        for _ in 0..16384 {
            if chunk(socket, frame.as_bytes()).is_err() {
                break;
            }
            sent += frame.len();
        }
        let _ = closed.send(sent);
    });
    let database = Database::new().await;
    database
        .add_provider(
            streaming::provider(Protocol::OpenAiChat, upstream.address.port(), 5000),
            Protocol::OpenAiChat,
            "upstream-model",
        )
        .await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let alias = alias(Protocol::Gemini, Protocol::OpenAiChat);
    let input = fixtures::request(Protocol::Gemini, &alias, "slow");
    let response = gateway.request(
        "POST",
        &streaming::path(Protocol::Gemini, &alias),
        "Content-Type: application/json\r\n",
        &serde_json::to_vec(&input).unwrap(),
    );
    assert_eq!(response.status, 200);
    // 持有连接却不读正文，让 TCP 发送缓冲最终填满；不能无限积累输出。
    let sent = receive
        .recv_timeout(std::time::Duration::from_secs(8))
        .unwrap();
    assert!(
        sent < 64 * 1024 * 1024,
        "背压必须在完整 128 MiB 源流之前到达 Provider"
    );
    drop(response);
    streaming::assert_logs(&gateway, 1);
    assert!(
        gateway.logs().contains("WriteTimedout"),
        "慢客户端必须由写超时终止"
    );
}
