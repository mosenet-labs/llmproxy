//! 独立登记与固定 Responses 转接；长轮询和有界 NDJSON 上传共用 Pingora 监听。
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use llmproxy_core::subscription::{
    Cancellations, Heartbeat, Lease, Presence, Registration, ResultFrame, Work,
};
use llmproxy_store::ProviderStore;
use pingora::{Error, ErrorType, Result, proxy::Session};
use pingora_http::ResponseHeader;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub fn matches(path: &str) -> bool {
    path.starts_with("/agents/v1/") || path.starts_with("/internal/subscriptions/")
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random() -> Result<String> {
    let mut b = [0; 32];
    getrandom::fill(&mut b)
        .map_err(|_| Error::explain(ErrorType::InternalError, "node random failed"))?;
    Ok(b.iter().map(|b| format!("{b:02x}")).collect())
}
fn secret_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}
fn failure() -> Box<Error> {
    Error::explain(ErrorType::InternalError, "subscription request interrupted")
}

enum Delivery {
    Data(Bytes),
    End,
}
struct Pending {
    head: Mutex<Option<oneshot::Sender<(u16, String)>>>,
    data: mpsc::Sender<Delivery>,
    uploading: AtomicBool,
}
struct Connection {
    token: String,
    work: mpsc::Sender<Work>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Work>>,
    cancelled: mpsc::Sender<String>,
    cancellations: Mutex<mpsc::Receiver<String>>,
    pending: Mutex<HashMap<String, Arc<Pending>>>,
    capacity: Arc<Semaphore>,
    closed: CancellationToken,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}

pub struct Hub {
    store: ProviderStore,
    registration_key: Option<String>,
    relay_key: Option<String>,
    presence: llmproxy_console::SubscriptionPresence,
    connections: Mutex<HashMap<String, Arc<Connection>>>,
    registration: tokio::sync::Mutex<()>,
}

impl Hub {
    pub fn new(store: ProviderStore, presence: llmproxy_console::SubscriptionPresence) -> Self {
        Self {
            store,
            presence,
            registration_key: std::env::var("LLMPROXY_SUBSCRIPTION_REGISTRATION_KEY").ok(),
            relay_key: std::env::var("LLMPROXY_SUBSCRIPTION_RELAY_KEY").ok(),
            connections: Mutex::default(),
            registration: tokio::sync::Mutex::new(()),
        }
    }
    fn connection(&self, node: &str, token: &str) -> Option<Arc<Connection>> {
        let online = self.presence.lock().ok()?.get(node)?.online_until > now();
        let connection = self.connections.lock().ok()?.get(node)?.clone();
        (online && !connection.closed.is_cancelled() && secret_eq(&connection.token, token))
            .then_some(connection)
    }
    pub async fn serve(&self, session: &mut Session) -> Result<()> {
        let token = session
            .req_header()
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .unwrap_or("")
            .to_owned();
        let path = session.req_header().uri.path().to_owned();
        let method = session.req_header().method.as_str().to_owned();
        if path == "/agents/v1/register" && method == "POST" {
            if !self
                .registration_key
                .as_ref()
                .is_some_and(|key| key.len() >= 32 && secret_eq(&token, key))
            {
                return status(session, 401).await;
            }
            let Some(body) = read_body(session, 128 * 1024).await? else {
                return status(session, 413).await;
            };
            let Ok(registration) = serde_json::from_slice::<Registration>(&body) else {
                return status(session, 400).await;
            };
            let _lock = self.registration.lock().await;
            if self
                .store
                .register_subscription(&registration)
                .await
                .is_err()
            {
                return status(session, 409).await;
            }
            let lease = random()?;
            let (work, receiver) = mpsc::channel(registration.concurrency);
            let (cancelled, cancellations) = mpsc::channel(64);
            let connection = Arc::new(Connection {
                token: lease.clone(),
                work,
                receiver: tokio::sync::Mutex::new(receiver),
                cancelled,
                cancellations: Mutex::new(cancellations),
                pending: Mutex::default(),
                capacity: Arc::new(Semaphore::new(registration.concurrency)),
                closed: CancellationToken::new(),
            });
            if let Some(old) = self
                .connections
                .lock()
                .unwrap()
                .insert(registration.node_id.clone(), connection)
            {
                old.closed.cancel();
            }
            self.presence.lock().unwrap().insert(
                registration.node_id.clone(),
                Presence {
                    online_until: now() + 60,
                    health: registration.health,
                },
            );
            let watched = self
                .connections
                .lock()
                .unwrap()
                .values()
                .find(|connection| connection.token == lease)
                .cloned()
                .unwrap();
            let presence = self.presence.clone();
            let node = registration.node_id.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! { _=watched.closed.cancelled()=>return,
                        _=tokio::time::sleep(Duration::from_secs(5))=> {
                            if !presence.lock().unwrap().get(&node).is_some_and(|p|p.online_until>now()) {
                                watched.closed.cancel();watched.pending.lock().unwrap().clear();return;
                            }
                        }
                    }
                }
            });
            return json(
                session,
                &Lease {
                    token: lease,
                    heartbeat_seconds: 10,
                },
            )
            .await;
        }
        let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if let ["agents", "v1", "node", node, operation, tail @ ..] = parts.as_slice() {
            let Some(connection) = self.connection(node, &token) else {
                return status(session, 401).await;
            };
            match (*operation, method.as_str(), tail) {
                ("heartbeat", "POST", []) => {
                    let Some(body) = read_body(session, 1024).await? else {
                        return status(session, 413).await;
                    };
                    let Ok(heartbeat) = serde_json::from_slice::<Heartbeat>(&body) else {
                        return status(session, 400).await;
                    };
                    let valid = {
                        let connections = self.connections.lock().unwrap();
                        let valid = connections
                            .get(*node)
                            .is_some_and(|current| Arc::ptr_eq(current, &connection))
                            && !connection.closed.is_cancelled();
                        if valid {
                            self.presence.lock().unwrap().insert(
                                (*node).to_owned(),
                                Presence {
                                    online_until: now() + 60,
                                    health: heartbeat.health,
                                },
                            );
                        }
                        valid
                    };
                    if !valid {
                        return status(session, 401).await;
                    }
                    let mut cancelled = Vec::new();
                    while let Ok(id) = connection.cancellations.lock().unwrap().try_recv() {
                        cancelled.push(id);
                    }
                    json(
                        session,
                        &Cancellations {
                            request_ids: cancelled,
                        },
                    )
                    .await
                }
                ("poll", "GET", []) => {
                    let mut receiver = connection.receiver.lock().await;
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                    let work = loop {
                        let work = tokio::select! { _=connection.closed.cancelled()=>None,
                        result=tokio::time::timeout_at(deadline,receiver.recv())=>result.ok().flatten() };
                        if work.as_ref().is_none_or(|work| {
                            connection
                                .pending
                                .lock()
                                .unwrap()
                                .contains_key(&work.request_id)
                        }) {
                            break work;
                        }
                    };
                    if let Some(work) = work {
                        json(session, &work).await
                    } else {
                        status(session, 204).await
                    }
                }
                ("result", "POST", [request_id]) => {
                    self.upload(session, &connection, request_id).await
                }
                _ => status(session, 404).await,
            }
        } else if let ["internal", "subscriptions", node, operation] = parts.as_slice() {
            if !self
                .relay_key
                .as_ref()
                .is_some_and(|key| key.len() >= 32 && secret_eq(&token, key))
            {
                return status(session, 401).await;
            }
            let nodes = self
                .store
                .subscription_nodes()
                .await
                .map_err(|_| failure())?;
            let Some(node_view) = nodes
                .into_iter()
                .find(|entry| entry.node_id == *node && entry.enabled)
            else {
                return status(session, 503).await;
            };
            if *operation == "models" && method == "GET" {
                let data:Vec<_>=node_view.models.iter().map(|id|serde_json::json!({"id":id,"object":"model","owned_by":"subscription-agent"})).collect();
                return json(session, &serde_json::json!({"object":"list","data":data})).await;
            }
            if *operation != "responses" || method != "POST" {
                return status(session, 404).await;
            }
            let connection = self.connections.lock().unwrap().get(*node).cloned();
            let Some(connection) = connection.filter(|connection| {
                !connection.closed.is_cancelled()
                    && self
                        .presence
                        .lock()
                        .unwrap()
                        .get(*node)
                        .is_some_and(|p| p.online_until > now())
            }) else {
                return status(session, 503).await;
            };
            self.forward(session, connection).await
        } else {
            status(session, 404).await
        }
    }

    async fn forward(&self, session: &mut Session, connection: Arc<Connection>) -> Result<()> {
        let Ok(_permit) = connection.capacity.clone().try_acquire_owned() else {
            return status(session, 429).await;
        };
        let Some(body) = read_body(session, 16 * 1024 * 1024).await? else {
            return status(session, 413).await;
        };
        let Ok(body) = serde_json::from_slice(&body) else {
            return status(session, 400).await;
        };
        let request_id = random()?;
        let (head, head_receiver) = oneshot::channel();
        let (data, mut receiver) = mpsc::channel(8);
        connection.pending.lock().unwrap().insert(
            request_id.clone(),
            Arc::new(Pending {
                head: Mutex::new(Some(head)),
                data,
                uploading: AtomicBool::new(false),
            }),
        );
        let mut guard = PendingGuard {
            connection: connection.clone(),
            request_id: request_id.clone(),
            finished: false,
        };
        if connection.work.try_send(Work { request_id, body }).is_err() {
            return status(session, 429).await;
        }
        session.downstream_session.set_abort_on_close(true);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        let header = tokio::select! { _=connection.closed.cancelled()=>None,
        _=session.downstream_session.read_body_or_idle(true)=>return Err(failure()),
        result=tokio::time::timeout_at(deadline,head_receiver)=>result.ok().and_then(std::result::Result::ok) };
        let Some((code, content_type)) = header else {
            return status(session, 502).await;
        };
        let mut header = ResponseHeader::build(code, None)?;
        header.insert_header("content-type", content_type)?;
        header.insert_header("cache-control", "no-store")?;
        session
            .write_response_header(Box::new(header), false)
            .await?;
        loop {
            let data = tokio::select! { _=connection.closed.cancelled()=>return Err(failure()),
            _=session.downstream_session.read_body_or_idle(true)=>return Err(failure()),
            result=tokio::time::timeout_at(deadline,receiver.recv())=>result.map_err(|_|failure())? };
            match data {
                Some(Delivery::Data(bytes)) => {
                    session.write_response_body(Some(bytes), false).await?
                }
                Some(Delivery::End) => {
                    guard.finished = true;
                    return session.write_response_body(None, true).await;
                }
                None => return Err(failure()),
            }
        }
    }

    async fn upload(
        &self,
        session: &mut Session,
        connection: &Arc<Connection>,
        id: &str,
    ) -> Result<()> {
        let pending = connection.pending.lock().unwrap().get(id).cloned();
        let Some(pending) = pending else {
            return status(session, 410).await;
        };
        if pending.uploading.swap(true, Ordering::SeqCst) {
            return status(session, 409).await;
        }
        let mut buffer = Vec::new();
        let mut headed = false;
        let mut ended = false;
        let operation = async {
            while let Some(chunk) = session.read_request_body().await? {
                for byte in &chunk {
                    if ended {
                        return Err(failure());
                    }
                    if *byte == b'\n' {
                        let frame: ResultFrame =
                            serde_json::from_slice(&buffer).map_err(|_| failure())?;
                        buffer.clear();
                        match frame {
                            ResultFrame::Head {
                                status,
                                content_type,
                            } if !headed
                                && (200..=599).contains(&status)
                                && matches!(
                                    content_type.as_str(),
                                    "application/json" | "text/event-stream"
                                ) =>
                            {
                                let tx = pending.head.lock().unwrap().take().ok_or_else(failure)?;
                                tx.send((status, content_type)).map_err(|_| failure())?;
                                headed = true;
                            }
                            ResultFrame::Data { base64 } if headed => {
                                let data = STANDARD.decode(base64).map_err(|_| failure())?;
                                if data.len() > 32 * 1024 {
                                    return Err(failure());
                                }
                                pending
                                    .data
                                    .send(Delivery::Data(Bytes::from(data)))
                                    .await
                                    .map_err(|_| failure())?;
                            }
                            ResultFrame::End if headed => ended = true,
                            _ => return Err(failure()),
                        }
                    } else {
                        buffer.push(*byte);
                        if buffer.len() > 64 * 1024 {
                            return Err(failure());
                        }
                    }
                }
            }
            if !ended || !buffer.is_empty() {
                return Err(failure());
            }
            pending
                .data
                .send(Delivery::End)
                .await
                .map_err(|_| failure())?;
            Ok(())
        };
        let result = tokio::select! { _=connection.closed.cancelled()=>Err(failure()),
        result=tokio::time::timeout(Duration::from_secs(300),operation)=>result.map_err(|_|failure())? };
        if result.is_err() {
            connection.pending.lock().unwrap().remove(id);
            return status(session, 410).await;
        }
        status(session, 204).await
    }
}

struct PendingGuard {
    connection: Arc<Connection>,
    request_id: String,
    finished: bool,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.connection
            .pending
            .lock()
            .unwrap()
            .remove(&self.request_id);
        if !self.finished
            && self
                .connection
                .cancelled
                .try_send(self.request_id.clone())
                .is_err()
        {
            self.connection.closed.cancel();
        }
    }
}

async fn read_body(session: &mut Session, limit: usize) -> Result<Option<Vec<u8>>> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut body = Vec::new();
        while let Some(chunk) = session.read_request_body().await? {
            if body.len() + chunk.len() > limit {
                session.set_keepalive(None);
                return Ok(None);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(body))
    })
    .await
    .map_err(|_| failure())?
}
async fn status(session: &mut Session, code: u16) -> Result<()> {
    session.set_keepalive(None);
    session.respond_error(code).await
}
async fn json(session: &mut Session, value: &impl serde::Serialize) -> Result<()> {
    let data = serde_json::to_vec(value).map_err(|_| failure())?;
    let mut header = ResponseHeader::build(200, None)?;
    header.insert_header("content-type", "application/json")?;
    header.insert_header("content-length", data.len().to_string())?;
    header.insert_header("cache-control", "no-store")?;
    session
        .write_response_header(Box::new(header), false)
        .await?;
    session
        .write_response_body(Some(Bytes::from(data)), true)
        .await
}
