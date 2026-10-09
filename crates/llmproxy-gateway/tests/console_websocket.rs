#[allow(dead_code)]
mod nonstream;
#[allow(dead_code)]
mod streaming;
mod support;

use futures_util::{SinkExt, StreamExt};
use llmproxy_core::protocol::Protocol;
use nonstream::{Database, MASTER_KEY};
use serde_json::{Value, json};
use support::{DEADLINE, Gateway, Mock, chunk, finish_chunks, sse_headers};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_tungstenite::{
    WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::{
            Role,
            frame::{
                Frame,
                coding::{Data, OpCode},
            },
        },
    },
};

type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

fn render(run: u64, path: &str, body: Value) -> Message {
    let headers = if path.contains("/shards/") {
        json!({"x-topcoat-identity":"_glY_FmvJupFutmO6b-Ysw","content-type":"application/json"})
    } else {
        json!({"x-topcoat-runtime":"true","content-type":"application/json"})
    };
    Message::Text(
        json!({"run":run,"method":"POST","path":path,"headers":headers,"body":body.to_string()})
            .to_string()
            .into(),
    )
}

async fn next_frame<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> Value {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let Message::Text(text) = socket
                .next()
                .await
                .expect("open socket")
                .expect("valid frame")
            {
                return serde_json::from_str(&text).unwrap();
            }
        }
    })
    .await
    .expect("runtime frame deadline")
}

async fn run_frame(socket: &mut Socket, run: u64) -> Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["run"] == run {
            return frame["frame"].clone();
        }
    }
}

fn request(gateway: &Gateway) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{}/ui/models", gateway.address)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "topcoat-runtime".parse().unwrap());
    request.headers_mut().insert(
        "origin",
        format!("http://{}", gateway.address).parse().unwrap(),
    );
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_runtime_connection_renders_and_enforces_request_guards() {
    let database = Database::new().await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    for (header, value, expected) in [
        ("origin", "https://other.example", 403),
        ("host", "other.example", 403),
        ("sec-websocket-protocol", "other", 400),
        ("sec-websocket-version", "12", 400),
        ("sec-websocket-key", "invalid", 400),
        ("upgrade", "other", 400),
        ("content-length", "1", 400),
    ] {
        let mut req = request(&gateway);
        req.headers_mut().insert(header, value.parse().unwrap());
        let error = connect_async(req).await.unwrap_err();
        match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status().as_u16(), expected)
            }
            error => panic!("unexpected handshake error: {error}"),
        }
    }
    let (mut socket, response) = connect_async(request(&gateway)).await.unwrap();
    assert_eq!(response.status().as_u16(), 101);
    assert_eq!(
        response.headers()["sec-websocket-protocol"],
        "topcoat-runtime"
    );
    for (run, path) in [(1, "/ui/models"), (2, "/ui/providers")] {
        socket.send(Message::Text(json!({"run":run,"method":"POST","path":path,"headers":{"x-topcoat-runtime":"true","content-type":"application/json"},"body":"{\"signals\":{}}"}).to_string().into())).await.unwrap();
    }
    let mut seen = std::collections::HashSet::new();
    tokio::time::timeout(DEADLINE, async {
        while seen.len() < 2 {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                let frame: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(frame["frame"]["t"], "error", "{text}");
                if text.contains("<main") {
                    seen.insert(frame["run"].as_u64().unwrap());
                }
            }
        }
    })
    .await
    .unwrap();
    for (run, path, headers, expected) in [
        (3, "/v1/responses", json!({}), 403),
        (4, "/ui/models", json!({"host":"other.example"}), 400),
        (
            5,
            "/ui/_topcoat/runtime/procedures/begin-chat",
            json!({"content-type":"application/json"}),
            403,
        ),
    ] {
        socket.send(Message::Text(json!({"run":run,"method":"POST","path":path,"headers":headers,"body":"[\"wrong-csrf\",\"session\",\"prompt\"]"}).to_string().into())).await.unwrap();
        tokio::time::timeout(DEADLINE, async {
            loop {
                if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    if frame["run"] == run {
                        assert_eq!(frame["frame"]["status"], expected, "{text}");
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
    }
    socket
        .send(render(
            6,
            "/ui/models",
            json!({"signals":{},"padding":"x".repeat(llmproxy_console::BODY_LIMIT)}),
        ))
        .await
        .unwrap();
    assert_eq!(run_frame(&mut socket, 6).await["status"], 413);
    socket
        .send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let Message::Pong(bytes) = socket.next().await.unwrap().unwrap() {
                assert_eq!(bytes.as_ref(), &[1, 2, 3]);
                break;
            }
        }
    })
    .await
    .unwrap();
    socket.close(None).await.unwrap();
    let (mut reopened, _) = connect_async(request(&gateway)).await.unwrap();
    reopened.close(None).await.unwrap();
    assert!(
        reqwest::get(format!("http://{}/ui/models", gateway.address))
            .await
            .unwrap()
            .status()
            .is_success()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_http_mode_keeps_shards_and_rejects_socket() {
    let database = Database::new().await;
    let gateway = Gateway::database_with_environment(
        &database.url,
        MASTER_KEY,
        &[("LLMPROXY_UI_WEBSOCKET", "false")],
    );
    let client = reqwest::Client::new();
    let url = format!("http://{}/ui/models", gateway.address);
    let html = client.get(&url).send().await.unwrap().text().await.unwrap();
    assert!(!html.contains("<!--::topcoat::connect-->"));
    let result = client
        .post(&url)
        .header("x-topcoat-runtime", "true")
        .json(&json!({"signals":{}}))
        .send()
        .await
        .unwrap();
    assert!(result.status().is_success());
    let error = connect_async(request(&gateway)).await.unwrap_err();
    assert!(
        matches!(error, tokio_tungstenite::tungstenite::Error::Http(response) if response.status().as_u16() == 403)
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_releases_an_idle_runtime_connection() {
    let database = Database::new().await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let (mut socket, _) = connect_async(request(&gateway)).await.unwrap();
    socket
        .send(render(1, "/ui/models", json!({"signals":{}})))
        .await
        .unwrap();
    assert_eq!(run_frame(&mut socket, 1).await["t"], "snapshot");
    gateway.request_shutdown();
    tokio::time::timeout(DEADLINE, async {
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await
    .expect("shutdown must close an idle socket without browser activity");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upgrade_preserves_preread_and_fragmented_frames() {
    let database = Database::new().await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let request = request(&gateway);
    let mut bytes = b"GET /ui/models HTTP/1.1\r\n".to_vec();
    for (name, value) in request.headers() {
        bytes.extend_from_slice(format!("{name}: {}\r\n", value.to_str().unwrap()).as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    let Message::Text(message) = render(
        1,
        "/ui/models",
        json!({"signals":{},"padding":"x".repeat(70_000)}),
    ) else {
        unreachable!()
    };
    for (index, part) in message.as_bytes().chunks(20_000).enumerate() {
        let mut frame = Frame::message(
            part.to_vec(),
            OpCode::Data(if index == 0 {
                Data::Text
            } else {
                Data::Continue
            }),
            (index + 1) * 20_000 >= message.len(),
        );
        frame.header_mut().mask = Some([1, 2, 3, 4]);
        frame.format(&mut bytes).unwrap();
    }
    let mut stream = TcpStream::connect(gateway.address).await.unwrap();
    // The first masked frame shares the HTTP header write; later writes split frames.
    for chunk in bytes.chunks(4093) {
        stream.write_all(chunk).await.unwrap();
    }
    let mut response = Vec::new();
    let boundary = tokio::time::timeout(DEADLINE, async {
        loop {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            response.extend_from_slice(&buffer[..count]);
            if let Some(boundary) = response.windows(4).position(|part| part == b"\r\n\r\n") {
                break boundary + 4;
            }
        }
    })
    .await
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 101"));
    let mut socket = WebSocketStream::from_partially_read(
        stream,
        response[boundary..].to_vec(),
        Role::Client,
        None,
    )
    .await;
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["run"], 1);
    assert_eq!(frame["frame"]["t"], "snapshot");
    assert!(frame["frame"]["html"].as_str().unwrap().contains("<main"));
    socket.close(None).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_run_capacity_cancellation_and_slow_reader_are_isolated() {
    // begin-chat reserves a live turn without making an upstream request.
    let (release, waiting) = std::sync::mpsc::channel();
    let waiting = std::sync::Mutex::new(waiting);
    let (upstream, _) = Mock::http(move |_, socket| {
        let (start, end) = streaming::frames(Protocol::OpenAiChat, false);
        sse_headers(socket);
        for frame in start {
            chunk(socket, &frame).unwrap();
        }
        waiting.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        for frame in end {
            chunk(socket, &frame).unwrap();
        }
        finish_chunks(socket);
    });
    let database = Database::new().await;
    database
        .add_provider(
            streaming::provider(Protocol::OpenAiChat, upstream.address.port(), 8000),
            Protocol::OpenAiChat,
            "mock",
        )
        .await;
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let client = reqwest::Client::new();
    let base = format!("http://{}/ui", gateway.address);
    let editor = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = editor
        .split_once("name=\"csrf\"")
        .unwrap()
        .1
        .split_once("value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    let page = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let session = page
        .split_once("data-chat-session=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    let procedure = |name: &str| client.post(format!("{base}/_topcoat/runtime/procedures/{name}"));
    assert_eq!(
        procedure("begin-chat")
            .json(&json!([csrf, session, "🧪".repeat(4000)]))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        true
    );
    let live = |run| {
        render(
            run,
            "/ui/_topcoat/runtime/shards/chat-history",
            json!({"args":[{"t":"Signal","id":"00000000000000000000000000000001","v":session},{"t":"Signal","id":"00000000000000000000000000000002","v":{"t":"usize","bits":64,"v":"0"}}],"signals":{}}),
        )
    };
    let (mut socket, _) = connect_async(request(&gateway)).await.unwrap();
    let mut body =
        nonstream::fixtures::request(Protocol::OpenAiChat, "internal-openai_chat", "parallel SSE");
    body["stream"] = json!(true);
    let response = client
        .post(format!("http://{}/v1/chat/completions", gateway.address))
        .bearer_auth(&gateway.api_key)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let mut sse = response.bytes_stream();
    let first = tokio::time::timeout(DEADLINE, sse.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("data:"));
    for run in 1..=64 {
        socket.send(live(run)).await.unwrap();
        assert_eq!(
            run_frame(&mut socket, run).await["t"],
            "snapshot",
            "run {run}"
        );
    }
    socket.send(live(65)).await.unwrap();
    assert_eq!(run_frame(&mut socket, 65).await["status"], 429);
    // Replacing a run does not consume capacity; stopping releases its slot.
    socket.send(live(1)).await.unwrap();
    assert_eq!(run_frame(&mut socket, 1).await["t"], "snapshot");
    socket
        .send(Message::Text(json!({"stop":1}).to_string().into()))
        .await
        .unwrap();
    socket.send(live(66)).await.unwrap();
    assert_eq!(run_frame(&mut socket, 66).await["t"], "snapshot");
    let (mut second, _) = connect_async(request(&gateway)).await.unwrap();
    second.send(live(1)).await.unwrap();
    assert_eq!(run_frame(&mut second, 1).await["t"], "snapshot");
    second.close(None).await.unwrap();
    release.send(()).unwrap();
    let tail = tokio::time::timeout(DEADLINE, async {
        let mut bytes = Vec::new();
        while let Some(chunk) = sse.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
        }
        bytes
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&tail).contains("[DONE]"));
    // Leave the first socket unread while it re-renders. Other HTTP work must proceed.
    for run in 2..=64 {
        socket.send(live(run)).await.unwrap();
    }
    let html = client
        .get(format!("{base}/chat"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("停止生成"));
    drop(socket); // EOF cancels renders, never the model turn.
    let (mut reopened, _) = connect_async(request(&gateway)).await.unwrap();
    reopened.send(live(1)).await.unwrap();
    assert_eq!(run_frame(&mut reopened, 1).await["t"], "snapshot");
    assert_eq!(
        procedure("stop-chat")
            .json(&json!([csrf, session]))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        true
    );
    let mut completed = false;
    while !completed {
        completed = run_frame(&mut reopened, 1).await["html"]
            .as_str()
            .is_some_and(|html| html.contains("已停止"));
    }
    reopened.close(None).await.unwrap();
}
