use crate::{
    Error,
    oauth::Session,
    responses::{Collector, prepare_codex_request, prepare_oauth_request},
};
use bytes::Bytes;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

pub const BODY_LIMIT: usize = 16 * 1024 * 1024;
pub const FRAME_LIMIT: usize = 16 * 1024 * 1024;

enum Backend {
    OAuth(Box<Session>),
    Codex(crate::native::Native),
}

pub struct Agent {
    #[cfg(test)]
    endpoint: String,
    backend: Backend,
    client: reqwest::Client,
    capacity: Arc<Semaphore>,
    pub concurrency: usize,
    pub models: Vec<String>,
    timeout: Duration,
    health: Arc<AtomicU8>,
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: mpsc::Receiver<Bytes>,
    pub cancel: CancellationToken,
}

impl Drop for Reply {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub fn error_body(code: &str, message: &str) -> Bytes {
    Bytes::from(
        serde_json::to_vec(
            &json!({"error":{"type":"invalid_request_error","code":code,"message":message}}),
        )
        .unwrap(),
    )
}

impl Reply {
    pub fn error(status: u16, code: &str, message: &str) -> Self {
        let (tx, rx) = mpsc::channel(1);
        let _ = tx.try_send(error_body(code, message));
        Self {
            status,
            content_type: "application/json",
            body: rx,
            cancel: CancellationToken::new(),
        }
    }
}

impl Agent {
    pub fn new(
        session: Session,
        concurrency: usize,
        models: Vec<String>,
        timeout: Duration,
    ) -> Result<Arc<Self>, Error> {
        Self::build(
            Backend::OAuth(Box::new(session)),
            concurrency,
            models,
            timeout,
        )
    }

    pub async fn codex(
        program: &std::path::Path,
        concurrency: usize,
        models: Vec<String>,
        timeout: Duration,
    ) -> Result<Arc<Self>, Error> {
        let native = crate::native::Native::start(program).await?;
        Self::build(Backend::Codex(native), concurrency, models, timeout)
    }

    pub fn backend_kind(&self) -> &'static str {
        match self.backend {
            Backend::OAuth(_) => "chatgpt-oauth",
            Backend::Codex(_) => "codex",
        }
    }

    fn build(
        backend: Backend,
        concurrency: usize,
        models: Vec<String>,
        timeout: Duration,
    ) -> Result<Arc<Self>, Error> {
        if concurrency == 0
            || concurrency > 64
            || timeout.is_zero()
            || models.is_empty()
            || models.len() > 256
            || models
                .iter()
                .any(|model| model.trim().is_empty() || model.len() > 256)
        {
            return Err("并发为 1–64、超时大于零且必须声明模型".into());
        }
        Ok(Arc::new(Self {
            #[cfg(test)]
            endpoint: "https://api.openai.com/v1/responses".into(),
            backend,
            client: crate::oauth::client()?,
            capacity: Arc::new(Semaphore::new(concurrency)),
            concurrency,
            models,
            timeout,
            health: Arc::new(AtomicU8::new(0)),
        }))
    }

    pub fn health(&self) -> llmproxy_core::subscription::Health {
        match self.health.load(Ordering::Relaxed) {
            1 => llmproxy_core::subscription::Health::Ready,
            2 => llmproxy_core::subscription::Health::Abnormal,
            _ => llmproxy_core::subscription::Health::Unknown,
        }
    }

    pub async fn respond(self: &Arc<Self>, bytes: &[u8], cancel: CancellationToken) -> Reply {
        if bytes.len() > BODY_LIMIT {
            return Reply::error(413, "body_too_large", "请求超过容量上限");
        }
        let request = match serde_json::from_slice(bytes) {
            Ok(request) => request,
            Err(_) => return Reply::error(400, "invalid_request", "Responses 请求无效"),
        };
        let prepared = match match &self.backend {
            Backend::OAuth(_) => prepare_oauth_request(request),
            Backend::Codex(_) => prepare_codex_request(request),
        } {
            Ok(request) => request,
            Err(error) => return Reply::error(400, error.param, error.message),
        };
        if prepared
            .upstream
            .model
            .as_option()
            .is_none_or(|model| !self.models.contains(model))
        {
            return Reply::error(404, "model_not_found", "模型不在当前节点声明的目录中");
        }
        let permit = match self.capacity.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Reply::error(429, "capacity_exceeded", "节点并发容量已满"),
        };
        let deadline = tokio::time::Instant::now() + self.timeout;
        #[cfg(not(test))]
        let endpoint = "https://api.openai.com/v1/responses";
        #[cfg(test)]
        let endpoint = &self.endpoint;
        let response = tokio::select! {
            _=cancel.cancelled()=>return Reply::error(499,"cancelled","请求已取消"),
            result=tokio::time::timeout_at(deadline,async {
                match &self.backend {
                    Backend::OAuth(session) => {
                        let token=session.token().await?;
                        self.client.post(endpoint).bearer_auth(token)
                            .header("accept", "text/event-stream").json(&prepared.upstream).send().await.map_err(|_| -> Error { "上游 Responses 连接失败".into() })
                    },
                    Backend::Codex(native) => native.response(serde_json::to_value(&prepared.upstream)?).await,
                }
            })=>match result {
                Ok(Ok(response))=>response,
                Ok(Err(_))=> {self.health.store(2,Ordering::Relaxed);return Reply::error(502,"backend_unavailable","认证或上游连接失败，请检查节点登录")},
                Err(_)=> {self.health.store(2,Ordering::Relaxed);return Reply::error(504,"timeout","上游响应超时")},
            }
        };
        if !response.status().is_success() {
            self.health.store(2, Ordering::Relaxed);
            let status =
                if response.status().is_client_error() || response.status().is_server_error() {
                    response.status().as_u16()
                } else {
                    502
                };
            let operation = async {
                let mut response = response;
                let mut body = Vec::new();
                while let Some(bytes) = response.chunk().await.map_err(|_| ())? {
                    if body.len() + bytes.len() > FRAME_LIMIT {
                        return Err(());
                    }
                    body.extend_from_slice(&bytes);
                }
                let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| ())?;
                if !value.is_object()
                    || !(value["error"].is_object() || value["detail"].is_string())
                {
                    return Err(());
                }
                Ok::<_, ()>(Bytes::from(body))
            };
            return tokio::select! {
                _=cancel.cancelled()=>Reply::error(499,"cancelled","请求已取消"),
                result=tokio::time::timeout_at(deadline,operation)=>match result {
                    Ok(Ok(body))=> {
                        let (tx,rx)=mpsc::channel(1); let _=tx.try_send(body);
                        Reply {status,content_type:"application/json",body:rx,cancel}
                    },
                    _=>Reply::error(status,"upstream_error","上游拒绝请求且未返回有效的 Responses 错误"),
                }
            };
        }
        // The real SIWC endpoint can omit Content-Type. The bounded Responses
        // decoder and terminal validation below still reject non-SSE bodies.
        if response.headers().get("content-type").is_some_and(|value| {
            !value
                .to_str()
                .is_ok_and(|value| value.starts_with("text/event-stream"))
        }) {
            return Reply::error(502, "invalid_upstream", "上游没有返回 Responses SSE");
        }
        if !prepared.downstream_stream {
            let operation = async {
                let mut response = response;
                let mut collector = Collector::new(FRAME_LIMIT);
                while let Some(bytes) = response.chunk().await.map_err(|_| "上游流式连接失败")?
                {
                    collector.push(&bytes)?;
                }
                let complete = collector.finish()?;
                self.health.store(
                    if complete.status.as_option().map(String::as_str) == Some("completed") {
                        1
                    } else {
                        2
                    },
                    Ordering::Relaxed,
                );
                serde_json::to_vec(&complete).map_err(|_| "无法序列化终态")
            };
            return tokio::select! {
                _=cancel.cancelled()=>Reply::error(499,"cancelled","请求已取消"),
                result=tokio::time::timeout_at(deadline,operation)=>match result {
                    Ok(Ok(bytes))=> {
                        let (tx,rx)=mpsc::channel(1); let _=tx.try_send(Bytes::from(bytes));
                        Reply {status:200,content_type:"application/json",body:rx,cancel}
                    },
                    Ok(Err(_))=>Reply::error(502,"upstream_interrupted","Responses 流未正常终止"),
                    Err(_)=>Reply::error(504,"timeout","上游响应超时"),
                }
            };
        }
        let (tx, rx) = mpsc::channel(8);
        let worker_cancel = cancel.clone();
        let health = self.health.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let operation = async {
                let mut response = response;
                let mut collector = Collector::new(FRAME_LIMIT);
                let mut decoder = llmproxy_core::protocol::stream::sse::Decoder::new(FRAME_LIMIT);
                let mut terminal = None;
                while let Some(bytes) = response.chunk().await.map_err(|_| "上游流式连接失败")?
                {
                    collector.push(&bytes)?;
                    for byte in &bytes {
                        if let Some(frame) = decoder.push(*byte)? {
                            if frame.data == b"[DONE]" {
                                continue;
                            }
                            let event = llmproxy_core::protocol::stream::sse::decode(
                                llmproxy_core::protocol::Protocol::OpenAiResponses,
                                &frame,
                            )?;
                            let is_terminal = matches!(&event,llmproxy_core::protocol::stream::Event::Responses(event)
                                if matches!(&**event,llmproxy_core::protocol::responses::response::Event::Known(event)
                                    if matches!(event.kind(),"response.completed"|"response.failed"|"response.incomplete")));
                            let output = Bytes::from(llmproxy_core::protocol::stream::sse::encode(
                                &event,
                                FRAME_LIMIT,
                            )?);
                            if is_terminal {
                                terminal = Some(output);
                            } else {
                                tx.send(output).await.map_err(|_| "客户端已断开")?;
                            }
                        }
                    }
                }
                let complete = collector.finish()?;
                health.store(
                    if complete.status.as_option().map(String::as_str) == Some("completed") {
                        1
                    } else {
                        2
                    },
                    Ordering::Relaxed,
                );
                tx.send(terminal.ok_or("未取得终态事件")?)
                    .await
                    .map_err(|_| "客户端已断开")?;
                Ok::<_, &'static str>(())
            };
            let result = tokio::select! {
                _=worker_cancel.cancelled()=>return,
                result=tokio::time::timeout_at(deadline,operation)=>result,
            };
            if !matches!(result, Ok(Ok(()))) {
                health.store(2, Ordering::Relaxed);
                let error = Bytes::from(
                    "event: error\ndata: {\"type\":\"error\",\"code\":\"upstream_interrupted\",\"message\":\"Responses 流失败或超时\",\"param\":null,\"sequence_number\":0}\n\n",
                );
                tokio::select! { _=worker_cancel.cancelled()=>{}, _=tokio::time::timeout(Duration::from_secs(2),tx.send(error))=>{} }
            }
        });
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body: rx,
            cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{credentials::StateDirectory, oauth::Credentials};
    use http_body_util::{BodyExt, Full};
    use hyper::{Response, service::service_fn};
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;

    #[tokio::test]
    async fn actual_http_streams_terminal_aggregation_capacity_and_cancellation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let captured = seen.clone();
        let mock = tokio::spawn(async move {
            loop {
                let socket = tokio::select! {_=shutdown.cancelled()=>break, socket=listener.accept()=>socket.unwrap().0};
                let captured = captured.clone();
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    let service = service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let captured = captured.clone();
                            async move {
                                assert_eq!(
                                    request.headers()["authorization"],
                                    "Bearer test-only-access"
                                );
                                let bytes = request.into_body().collect().await.unwrap().to_bytes();
                                let request: serde_json::Value =
                                    serde_json::from_slice(&bytes).unwrap();
                                assert_eq!(request["store"], false);
                                assert_eq!(request["stream"], true);
                                captured.lock().await.push(request.clone());
                                if request["input"][0]["content"] == "rejected" {
                                    return Ok::<_, Infallible>(Response::builder().status(400)
                                        .header("content-type", "application/json")
                                        .body(Full::new(Bytes::from_static(b"{\"error\":{\"type\":\"invalid_request_error\",\"code\":\"unsupported_parameter\",\"param\":\"temperature\",\"message\":\"Parameter unsupported\",\"future\":true}}"))).unwrap());
                                }
                                if request["input"][0]["content"] == "slow" {
                                    tokio::time::sleep(Duration::from_secs(30)).await;
                                }
                                let response = json!({"id":"resp_test","object":"response","created_at":1,"model":"m","status":"completed","output":[],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}});
                                let body = if request["input"][0]["content"] == "buffered" {
                                    let mut body = String::new();
                                    for index in 0..32 {
                                        body.push_str(&format!(
                                            "event: response.output_text.delta\ndata: {}\n\n",
                                            json!({"type":"response.output_text.delta","sequence_number":index,"item_id":"msg_test","output_index":0,"content_index":0,"delta":"a","logprobs":[]})
                                        ));
                                    }
                                    body.push_str(&format!(
                                        "event: response.completed\ndata: {}\n\n",
                                        json!({"type":"response.completed","sequence_number":32,"response":response})
                                    ));
                                    body
                                } else if request["input"][0]["content"] == "truncated" {
                                    "event: response.created\ndata: {\"type\":\"response.created\",\"sequence_number\":0,\"response\":{\"id\":\"resp_test\",\"status\":\"in_progress\",\"output\":[]}}\n\n".into()
                                } else {
                                    format!(
                                        "event: response.completed\ndata: {}\n\n",
                                        json!({"type":"response.completed","sequence_number":1,"response":response})
                                    )
                                };
                                let builder = Response::builder();
                                let builder = match request["input"][0]["content"].as_str() {
                                    Some("missing-type" | "missing-type-invalid") => builder,
                                    Some("wrong-type") => {
                                        builder.header("content-type", "application/json")
                                    }
                                    _ => builder.header("content-type", "text/event-stream"),
                                };
                                let body =
                                    if request["input"][0]["content"] == "missing-type-invalid" {
                                        "{}".to_owned()
                                    } else {
                                        body
                                    };
                                Ok::<_, Infallible>(
                                    builder.body(Full::new(Bytes::from(body))).unwrap(),
                                )
                            }
                        },
                    );
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service);
                    tokio::select! {_=shutdown.cancelled()=>{}, _=connection=>{}}
                });
            }
        });
        let path = std::env::temp_dir().join(format!(
            "llmproxy-agent-test-{}",
            crate::random_id().unwrap()
        ));
        let directory = Arc::new(StateDirectory::open(path.clone()).unwrap());
        directory
            .write(
                "oauth.json",
                &Credentials {
                    client_id: "test-client".into(),
                    subject: "test-subject".into(),
                    host_id: "test-host".into(),
                    access_token: "test-only-access".into(),
                    refresh_token: "test-only-refresh".into(),
                    id_token: "test-only-id".into(),
                    scopes: "chatgpt.tokens.use.direct".into(),
                    expires_at: crate::now().unwrap() + 3600,
                },
            )
            .unwrap();
        let mut agent = Agent::new(
            Session::load(directory).unwrap(),
            1,
            vec!["m".into()],
            Duration::from_secs(5),
        )
        .unwrap();
        Arc::get_mut(&mut agent).unwrap().client =
            reqwest::Client::builder().no_proxy().build().unwrap();
        Arc::get_mut(&mut agent).unwrap().endpoint = format!("http://{address}/v1/responses");
        let local_address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let local_stop = CancellationToken::new();
        let local_shutdown = local_stop.clone();
        let local_agent = agent.clone();
        let key = "test-local-key".repeat(4);
        let local_key = key.clone();
        let server = tokio::spawn(async move {
            crate::http::serve(local_agent, local_address, local_key, local_shutdown)
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let url = format!("http://{local_address}");
        for _ in 0..100 {
            if client.get(format!("{url}/health")).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let health: serde_json::Value = client
            .get(format!("{url}/health"))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["backend"], agent.backend_kind());
        for (input, status) in [
            ("missing-type", 200),
            ("missing-type-invalid", 502),
            ("wrong-type", 502),
        ] {
            let response = client
                .post(format!("{url}/v1/responses"))
                .bearer_auth(&key)
                .json(&json!({"model":"m","input":input,"stream":false,"store":false}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status, "{input}");
        }
        assert_eq!(
            client
                .get(format!("{url}/health"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .get(format!("{url}/v1/models"))
                .bearer_auth(&key)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let request = |input: &str, stream: bool| {
            serde_json::to_vec(&json!({"model":"m","input":input,"stream":stream})).unwrap()
        };
        let mut rejected = agent
            .respond(&request("rejected", true), CancellationToken::new())
            .await;
        assert_eq!(rejected.status, 400);
        let body: serde_json::Value =
            serde_json::from_slice(&rejected.body.recv().await.unwrap()).unwrap();
        assert_eq!(body["error"]["param"], "temperature");
        assert_eq!(body["error"]["code"], "unsupported_parameter");
        assert_eq!(body["error"]["future"], true);
        drop(rejected);
        let mut response = agent
            .respond(&request("hello", false), CancellationToken::new())
            .await;
        assert_eq!(response.status, 200);
        let body = response.body.recv().await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["usage"]["total_tokens"],
            5
        );
        drop(response);
        let response = client
            .post(format!("{url}/v1/responses"))
            .bearer_auth(&key)
            .body(request("wire", false))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.bytes().await.unwrap()).unwrap()
                ["status"],
            "completed"
        );
        let mut response = agent
            .respond(&request("hello", true), CancellationToken::new())
            .await;
        assert_eq!(response.status, 200);
        let body = response.body.recv().await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("response.completed"));
        drop(response);
        // 客户端不读取输出时，工作线程在有界队列上等待；断开必须释放容量。
        let response = agent
            .respond(&request("buffered", true), CancellationToken::new())
            .await;
        assert_eq!(response.status, 200);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(agent.capacity.available_permits(), 0);
        drop(response);
        for _ in 0..100 {
            if agent.capacity.available_permits() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(agent.capacity.available_permits(), 1);
        let response = agent
            .respond(&request("truncated", false), CancellationToken::new())
            .await;
        assert_eq!(response.status, 502);
        drop(response);
        let cancel = CancellationToken::new();
        let worker = agent.clone();
        let worker_cancel = cancel.clone();
        let pending =
            tokio::spawn(
                async move { worker.respond(&request("slow", false), worker_cancel).await },
            );
        for _ in 0..100 {
            if agent.capacity.available_permits() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            agent
                .respond(&request("busy", false), CancellationToken::new())
                .await
                .status,
            429
        );
        assert_eq!(
            client
                .post(format!("{url}/v1/responses"))
                .bearer_auth(&key)
                .body(request("busy", false))
                .send()
                .await
                .unwrap()
                .status(),
            429
        );
        cancel.cancel();
        assert_eq!(pending.await.unwrap().status, 499);
        assert_eq!(agent.capacity.available_permits(), 1);
        assert_eq!(
            agent
                .respond(&request("recovered", false), CancellationToken::new())
                .await
                .status,
            200
        );
        assert!(
            !seen
                .lock()
                .await
                .iter()
                .any(|value| value["input"][0]["content"] == "busy")
        );
        stop.cancel();
        mock.await.unwrap();
        local_stop.cancel();
        server.await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
}
