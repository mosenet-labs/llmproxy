use crate::{
    Error,
    credentials::StateDirectory,
    http::limited_response,
    random_id,
    service::{Agent, Reply},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use llmproxy_core::subscription::{
    self, Cancellations, Heartbeat, Lease, Registration, ResultFrame, Work,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Serialize, Deserialize)]
struct Identity {
    node_id: String,
    node_key: String,
}
pub struct Remote {
    pub url: Url,
    pub registration_key: String,
    pub name: String,
}

impl Remote {
    pub fn validate(&self) -> Result<(), Error> {
        if self.url.scheme() != "https"
            && !(self.url.scheme() == "http"
                && matches!(
                    self.url.host_str(),
                    Some("127.0.0.1" | "localhost" | "[::1]")
                ))
        {
            return Err("公网反向连接必须使用 HTTPS".into());
        }
        if !self.url.username().is_empty()
            || self.url.password().is_some()
            || self.url.query().is_some()
            || self.url.fragment().is_some()
            || self.url.path() != "/"
            || self.registration_key.len() < 32
            || self.name.len() > 128
        {
            return Err("远程地址、注册密钥或节点名称无效".into());
        }
        Ok(())
    }
}

pub async fn run(
    agent: Arc<Agent>,
    directory: Arc<StateDirectory>,
    remote: Remote,
    shutdown: CancellationToken,
) -> Result<(), Error> {
    remote.validate()?;
    let identity = directory
        .read::<Identity>("node.json")?
        .unwrap_or(Identity {
            node_id: random_id()?,
            node_key: random_id()?,
        });
    directory.write("node.json", &identity)?;
    let client = crate::oauth::client()?;
    let mut backoff = 2;
    let mut announced = false;
    let mut reconnect_reported = false;
    let mut last_reconnect_notice: Option<std::time::Instant> = None;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let registration = Registration {
            version: subscription::VERSION,
            node_id: identity.node_id.clone(),
            node_key: identity.node_key.clone(),
            name: remote.name.clone(),
            backend: agent.backend_kind().into(),
            models: agent.models.clone(),
            concurrency: agent.concurrency,
            health: agent.health(),
        };
        let lease = tokio::select! { _=shutdown.cancelled()=>return Ok(()),
            response=tokio::time::timeout(Duration::from_secs(30),async {
                let url=remote.url.join("agents/v1/register").map_err(|_| "注册地址无效".to_owned())?;
                let response=client.post(url).bearer_auth(&remote.registration_key)
                    .json(&registration).send().await.map_err(|_| "无法连接远程 llmproxy".to_owned())?;
                if !response.status().is_success() {
                    return Err(format!("HTTP {}；请检查 llmproxy 服务端注册配置与注册密钥", response.status().as_u16()));
                }
                let body=limited_response(response,16*1024).await.map_err(|_| "注册响应读取失败".to_owned())?;
                serde_json::from_slice::<Lease>(&body).map_err(|_| "节点租约无效".to_owned())
            })=>match response {
                Ok(result)=>result,
                Err(_)=>Err("注册请求超时".to_owned()),
            }
        };
        let lease = match lease {
            Ok(lease) => Some(lease),
            Err(reason) => {
                eprintln!("节点注册失败：{reason}；{backoff} 秒后重试。");
                None
            }
        };
        if let Some(lease) = lease {
            if lease.token.len() != 64
                || lease.heartbeat_seconds == 0
                || lease.heartbeat_seconds > 20
            {
                return Err("远程返回的租约无效".into());
            }
            if !announced {
                println!(
                    "节点已注册：{}；服务准入由远程 llmproxy 控制。",
                    &identity.node_id[..12]
                );
                announced = true;
            } else if reconnect_reported {
                println!("反向连接已恢复。");
                reconnect_reported = false;
            }
            backoff = 2;
            let closed = CancellationToken::new();
            let requests = Arc::new(Mutex::new(HashMap::<String, CancellationToken>::new()));
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..agent.concurrency {
                let agent = agent.clone();
                let client = client.clone();
                let url = remote.url.clone();
                let node = identity.node_id.clone();
                let token = lease.token.clone();
                let closed = closed.clone();
                let requests = requests.clone();
                tasks.spawn(async move {
                    let result =
                        worker(agent, client, url, node, token, closed.clone(), requests).await;
                    if result.is_err() {
                        closed.cancel();
                    }
                });
            }
            let operation = async {
                loop {
                    tokio::time::sleep(Duration::from_secs(lease.heartbeat_seconds)).await;
                    let response = client
                        .post(
                            remote
                                .url
                                .join(&format!("agents/v1/node/{}/heartbeat", identity.node_id))?,
                        )
                        .bearer_auth(&lease.token)
                        .json(&Heartbeat {
                            health: agent.health(),
                        })
                        .timeout(Duration::from_secs(15))
                        .send()
                        .await?;
                    if !response.status().is_success() {
                        return Err("节点心跳失效".into());
                    }
                    let cancellations: Cancellations =
                        serde_json::from_slice(&limited_response(response, 64 * 1024).await?)
                            .map_err(|_| "取消列表无效")?;
                    for id in cancellations.request_ids {
                        if let Some(cancel) = requests.lock().unwrap().get(&id) {
                            cancel.cancel();
                        }
                    }
                }
                #[allow(unreachable_code)]
                Ok::<_, Error>(())
            };
            tokio::select! { _=shutdown.cancelled()=>{}, _=closed.cancelled()=>{}, _=operation=>{} }
            closed.cancel();
            for cancel in requests.lock().unwrap().values() {
                cancel.cancel();
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            if shutdown.is_cancelled() {
                return Ok(());
            }
            if last_reconnect_notice.is_none_or(|last| last.elapsed() >= Duration::from_secs(60)) {
                eprintln!(
                    "反向连接失效，{backoff} 秒后重连；不会重放推理请求（重复提示每分钟最多一次）。"
                );
                last_reconnect_notice = Some(std::time::Instant::now());
                reconnect_reported = true;
            }
        }
        tokio::select! {_=shutdown.cancelled()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(backoff))=>{}}
        backoff = (backoff * 2).min(30);
    }
}

async fn worker(
    agent: Arc<Agent>,
    client: reqwest::Client,
    url: Url,
    node: String,
    token: String,
    closed: CancellationToken,
    requests: Arc<Mutex<HashMap<String, CancellationToken>>>,
) -> Result<(), Error> {
    loop {
        let operation = async {
            let response = client
                .get(url.join(&format!("agents/v1/node/{node}/poll"))?)
                .bearer_auth(&token)
                .timeout(Duration::from_secs(30))
                .send()
                .await?;
            if response.status() == 204 {
                return Ok(());
            }
            if !response.status().is_success() {
                return Err("反向派发失败".into());
            }
            let work: Work = serde_json::from_slice(
                &limited_response(response, crate::service::BODY_LIMIT + 1024).await?,
            )
            .map_err(|_| "派发请求无效")?;
            if work.request_id.len() != 64
                || !work.request_id.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("派发请求 ID 无效".into());
            }
            let cancel = closed.child_token();
            if requests
                .lock()
                .unwrap()
                .insert(work.request_id.clone(), cancel.clone())
                .is_some()
            {
                return Err("派发请求 ID 重复".into());
            }
            let _guard = RequestGuard {
                requests: requests.clone(),
                id: work.request_id.clone(),
                cancel: cancel.clone(),
            };
            let bytes = serde_json::to_vec(&work.body)?;
            let reply = agent.respond(&bytes, cancel.child_token()).await;
            let upload = Upload {
                reply,
                head: false,
                ended: false,
                buffer: VecDeque::new(),
            };
            let stream = futures_util::stream::unfold(upload, |mut state| async move {
                let frame = if !state.head {
                    state.head = true;
                    ResultFrame::Head {
                        status: state.reply.status,
                        content_type: state.reply.content_type.to_owned(),
                    }
                } else if let Some(bytes) = state.buffer.pop_front() {
                    ResultFrame::Data {
                        base64: STANDARD.encode(bytes),
                    }
                } else if state.ended {
                    return None;
                } else if let Some(bytes) = state.reply.body.recv().await {
                    for chunk in bytes.chunks(32 * 1024) {
                        state.buffer.push_back(Bytes::copy_from_slice(chunk));
                    }
                    ResultFrame::Data {
                        base64: STANDARD.encode(state.buffer.pop_front().unwrap_or_default()),
                    }
                } else {
                    state.ended = true;
                    ResultFrame::End
                };
                let mut bytes = serde_json::to_vec(&frame).unwrap();
                bytes.push(b'\n');
                Some((Ok::<_, std::io::Error>(Bytes::from(bytes)), state))
            });
            let result = tokio::select! { _=cancel.cancelled()=>return Ok(()),
                response=client.post(url.join(&format!("agents/v1/node/{node}/result/{}",work.request_id))?).bearer_auth(&token)
                    .header("content-type","application/x-ndjson").body(reqwest::Body::wrap_stream(stream))
                    .timeout(Duration::from_secs(310)).send()=>response?
            };
            if !result.status().is_success() && result.status() != 410 {
                return Err("反向结果交付失败".into());
            }
            Ok::<_, Error>(())
        };
        tokio::select! { _=closed.cancelled()=>return Ok(()), result=operation=>result? }
    }
}
struct Upload {
    reply: Reply,
    head: bool,
    ended: bool,
    buffer: VecDeque<Bytes>,
}
struct RequestGuard {
    requests: Arc<Mutex<HashMap<String, CancellationToken>>>,
    id: String,
    cancel: CancellationToken,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.requests.lock().unwrap().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::{Credentials, Session};
    use http_body_util::{BodyExt, Full};
    use hyper::{Response, service::service_fn};
    use hyper_util::rt::TokioIo;
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn outgoing_worker_registers_and_uploads_without_sending_oauth_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let key = "test-register-key".repeat(4);
        let expected_key = key.clone();
        let (results, mut received) = tokio::sync::mpsc::channel(2);
        let issued = Arc::new(AtomicBool::new(false));
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            loop {
                let socket = tokio::select! {_=stop.cancelled()=>break, socket=listener.accept()=>socket.unwrap().0};
                let results = results.clone();
                let issued = issued.clone();
                let key = expected_key.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    let service =
                        service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            let results = results.clone();
                            let issued = issued.clone();
                            let key = key.clone();
                            async move {
                                let path = request.uri().path().to_owned();
                                let token = request.headers()["authorization"]
                                    .to_str()
                                    .unwrap()
                                    .to_owned();
                                let body = request.into_body().collect().await.unwrap().to_bytes();
                                let (status, body) = if path == "/agents/v1/register" {
                                    assert_eq!(token, format!("Bearer {key}"));
                                    assert!(
                                        !String::from_utf8_lossy(&body).contains("private-access")
                                    );
                                    let registration: Registration =
                                        serde_json::from_slice(&body).unwrap();
                                    assert_eq!(registration.name, "worker-test");
                                    assert_eq!(registration.backend, "chatgpt-oauth");
                                    (
                                        200,
                                        serde_json::to_vec(&Lease {
                                            token: "d".repeat(64),
                                            heartbeat_seconds: 1,
                                        })
                                        .unwrap(),
                                    )
                                } else {
                                    assert_eq!(token, format!("Bearer {}", "d".repeat(64)));
                                    if path.ends_with("/poll") {
                                        if !issued.swap(true, Ordering::SeqCst) {
                                            (
                                                200,
                                                serde_json::to_vec(&Work {
                                                    request_id: "e".repeat(64),
                                                    body: serde_json::json!({"model":"m"}),
                                                })
                                                .unwrap(),
                                            )
                                        } else {
                                            tokio::time::sleep(Duration::from_millis(100)).await;
                                            (204, vec![])
                                        }
                                    } else if path.contains("/result/") {
                                        results.send(body.to_vec()).await.unwrap();
                                        (204, vec![])
                                    } else {
                                        (
                                            200,
                                            serde_json::to_vec(&Cancellations {
                                                request_ids: vec![],
                                            })
                                            .unwrap(),
                                        )
                                    }
                                };
                                Ok::<_, Infallible>(
                                    Response::builder()
                                        .status(status)
                                        .header("content-type", "application/json")
                                        .body(Full::new(Bytes::from(body)))
                                        .unwrap(),
                                )
                            }
                        });
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service);
                    tokio::select! {_=stop.cancelled()=>{}, _=connection=>{}}
                });
            }
        });
        let path =
            std::env::temp_dir().join(format!("llmproxy-worker-test-{}", random_id().unwrap()));
        let directory = Arc::new(StateDirectory::open(path.clone()).unwrap());
        directory
            .write(
                "oauth.json",
                &Credentials {
                    client_id: "test-client".into(),
                    subject: "test-subject".into(),
                    host_id: "test-host".into(),
                    access_token: "private-access".into(),
                    refresh_token: "private-refresh".into(),
                    id_token: "private-id".into(),
                    scopes: "chatgpt.tokens.use.direct".into(),
                    expires_at: crate::now().unwrap() + 3600,
                },
            )
            .unwrap();
        let agent = Agent::new(
            Session::load(directory.clone()).unwrap(),
            1,
            vec!["m".into()],
            Duration::from_secs(5),
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let state = directory.clone();
        let worker = tokio::spawn(async move {
            run(
                agent,
                state,
                Remote {
                    url: format!("http://{address}/").parse().unwrap(),
                    registration_key: key,
                    name: "worker-test".into(),
                },
                worker_cancel,
            )
            .await
            .unwrap();
        });
        let output = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .unwrap()
            .unwrap();
        let frames: Vec<ResultFrame> = output
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert!(matches!(
            frames.first(),
            Some(ResultFrame::Head { status: 400, .. })
        ));
        assert!(matches!(frames.last(), Some(ResultFrame::End)));
        assert!(
            matches!(&frames[1],ResultFrame::Data{base64} if String::from_utf8_lossy(&STANDARD.decode(base64).unwrap()).contains("input"))
        );
        let identity = directory.read::<Identity>("node.json").unwrap().unwrap();
        assert_eq!(identity.node_id.len(), 64);
        assert!(!String::from_utf8_lossy(&output).contains("private-access"));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        shutdown.cancel();
        server.await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
}
