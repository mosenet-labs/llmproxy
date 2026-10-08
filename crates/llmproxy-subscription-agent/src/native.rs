//! 受控 Codex 的无会话 RPC。唯一读取任务保持帧边界，慢请求不会阻塞其他请求。
use crate::{
    Error,
    codex_rpc::{Client, verify_policy},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    process::ChildStdin,
    sync::{mpsc, oneshot},
};

type Head = oneshot::Sender<Result<Value, &'static str>>;
struct Pending {
    head: Option<Head>,
    body: mpsc::Sender<Bytes>,
    state: Arc<State>,
}
#[derive(Default)]
struct State {
    failed: AtomicBool,
    done: AtomicBool,
    cancelled: AtomicBool,
}
struct Core {
    writer: Arc<tokio::sync::Mutex<ChildStdin>>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Pending>>,
    controls: Mutex<HashSet<u64>>,
    failed: AtomicBool,
    stop: tokio_util::sync::CancellationToken,
}
pub struct Native {
    core: Arc<Core>,
    reader: tokio::task::AbortHandle,
}
impl Drop for Native {
    fn drop(&mut self) {
        self.core.fail_all();
        self.reader.abort();
    }
}

impl Native {
    pub async fn start(program: &Path) -> Result<Self, Error> {
        crate::doctor::require_controlled_transport(program)
            .await
            .map_err(|error| -> Error { error.into() })?;
        let mut client = Client::spawn(program).await?;
        let capabilities = client.call("llmproxy/capabilities", json!({})).await?;
        if let Err(error) = verify_policy(&capabilities) {
            client.shutdown().await;
            return Err(error);
        }
        let core = Arc::new(Core {
            writer: Arc::new(tokio::sync::Mutex::new(client.writer)),
            next: AtomicU64::new(client.next_id),
            pending: Mutex::new(HashMap::new()),
            controls: Mutex::new(HashSet::new()),
            failed: AtomicBool::new(false),
            stop: tokio_util::sync::CancellationToken::new(),
        });
        let read_core = core.clone();
        let reader = tokio::spawn(async move {
            let mut child = client.child;
            let mut reader = client.reader;
            let operation = async {
                loop {
                    let mut frame = Vec::new();
                    let size = (&mut reader)
                        .take((crate::service::FRAME_LIMIT + 1) as u64)
                        .read_until(b'\n', &mut frame)
                        .await
                        .map_err(|_| ())?;
                    if size == 0 || size > crate::service::FRAME_LIMIT || !frame.ends_with(b"\n") {
                        return Err::<(), ()>(());
                    }
                    let message: Value = serde_json::from_slice(&frame).map_err(|_| ())?;
                    read_core.dispatch(message)?;
                }
            };
            tokio::select! { _ = read_core.stop.cancelled() => {}, _ = operation => {} }
            read_core.fail_all();
            let _ = child.kill().await;
            let _ = child.wait().await;
        })
        .abort_handle();
        Ok(Self { core, reader })
    }

    pub async fn response(&self, request: Value) -> Result<reqwest::Response, Error> {
        if self.core.failed.load(Ordering::Acquire) {
            return Err("Codex 连接不可用".into());
        }
        let id = self.core.id()?;
        let rpc = json!({"id":id,"method":"llmproxy/responses","params":{"request":request}});
        if serde_json::to_vec(&rpc)?.len() > crate::service::FRAME_LIMIT {
            return Err("Codex RPC 请求超过容量上限".into());
        }
        // 先占有写锁，再建立取消守卫，保证取消帧不会越过尚未写出的请求。
        let writer = self.core.writer.clone().lock_owned().await;
        if self.core.failed.load(Ordering::Acquire) {
            return Err("Codex 连接不可用".into());
        }
        let state = Arc::new(State::default());
        let (head_tx, head_rx) = oneshot::channel();
        let (body_tx, body_rx) = mpsc::channel(8);
        {
            let mut pending = self.core.pending.lock().unwrap();
            if pending.len() >= 64 {
                return Err("Codex 请求容量已满".into());
            }
            pending.insert(
                id,
                Pending {
                    head: Some(head_tx),
                    body: body_tx,
                    state: state.clone(),
                },
            );
        }
        let guard = Guard {
            core: self.core.clone(),
            state: state.clone(),
            id,
        };
        self.core.write(writer, rpc).await?;
        let head = head_rx
            .await
            .map_err(|_| "Codex 响应头连接中断")?
            .map_err(|_| "Codex 请求失败")?;
        let status = head["status"]
            .as_u64()
            .filter(|v| (100..=599).contains(v))
            .ok_or("Codex 状态码无效")? as u16;
        let content_type = match &head["contentType"] {
            Value::Null => None,
            Value::String(value) => Some(value.as_str()),
            _ => return Err("Codex 响应类型无效".into()),
        };
        let stream = futures_util::stream::unfold(
            (body_rx, guard, false),
            |(mut receiver, guard, ended)| async move {
                if ended {
                    return None;
                }
                if guard.state.failed.load(Ordering::Acquire) {
                    return Some((
                        Err(std::io::Error::other("Codex 响应流中断")),
                        (receiver, guard, true),
                    ));
                }
                match receiver.recv().await {
                    Some(bytes) => Some((Ok(bytes), (receiver, guard, false))),
                    None if guard.state.failed.load(Ordering::Acquire)
                        || !guard.state.done.load(Ordering::Acquire) =>
                    {
                        Some((
                            Err(std::io::Error::other("Codex 响应流未正常结束")),
                            (receiver, guard, true),
                        ))
                    }
                    None => None,
                }
            },
        );
        let builder = hyper::Response::builder().status(status);
        let builder = match content_type {
            Some(value) => builder.header("content-type", value),
            None => builder,
        };
        let response = builder.body(reqwest::Body::wrap_stream(stream))?;
        Ok(response.into())
    }
}

struct Guard {
    core: Arc<Core>,
    state: Arc<State>,
    id: u64,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.core.cancel(self.id, &self.state);
    }
}

impl Core {
    fn id(&self) -> Result<u64, Error> {
        self.next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "Codex RPC ID 耗尽".into())
    }
    async fn send(self: &Arc<Self>, value: Value) -> Result<(), Error> {
        let writer =
            tokio::time::timeout(Duration::from_secs(30), self.writer.clone().lock_owned())
                .await
                .map_err(|_| "Codex RPC 写锁超时")?;
        self.write(writer, value).await
    }

    async fn write(
        self: &Arc<Self>,
        mut writer: tokio::sync::OwnedMutexGuard<ChildStdin>,
        value: Value,
    ) -> Result<(), Error> {
        let mut bytes = serde_json::to_vec(&value)?;
        if bytes.len() > crate::service::FRAME_LIMIT {
            return Err("Codex RPC 请求超过容量上限".into());
        }
        bytes.push(b'\n');
        // 写入任务独立于请求取消；半个 JSON 帧不能与后续请求拼接。
        let core = self.clone();
        tokio::spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(30), async {
                if core.failed.load(Ordering::Acquire) {
                    return Err(std::io::Error::other("连接已关闭"));
                }
                writer.write_all(&bytes).await?;
                writer.flush().await
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                core.fail_all();
                return Err::<(), Error>("Codex RPC 写入失败".into());
            }
            Ok(())
        })
        .await
        .map_err(|_| "Codex RPC 写入任务中断")?
    }
    fn fail_all(&self) {
        self.failed.store(true, Ordering::Release);
        self.stop.cancel();
        for (_, mut pending) in self.pending.lock().unwrap().drain() {
            pending.state.failed.store(true, Ordering::Release);
            if let Some(head) = pending.head.take() {
                let _ = head.send(Err("连接中断"));
            }
        }
        self.controls.lock().unwrap().clear();
    }
    fn cancel(self: &Arc<Self>, target: u64, state: &State) {
        if state.done.load(Ordering::Acquire)
            || state.cancelled.swap(true, Ordering::AcqRel)
            || self.failed.load(Ordering::Acquire)
        {
            return;
        }
        let Ok(id) = self.id() else {
            self.failed.store(true, Ordering::Release);
            self.stop.cancel();
            return;
        };
        {
            let mut controls = self.controls.lock().unwrap();
            if controls.len() >= 64 {
                drop(controls);
                self.failed.store(true, Ordering::Release);
                self.stop.cancel();
                return;
            }
            controls.insert(id);
        }
        let core = self.clone();
        tokio::spawn(async move {
            if core
                .send(json!({"id":id,"method":"llmproxy/cancel","params":{"requestId":target}}))
                .await
                .is_err()
            {
                core.fail_all();
            }
        });
    }
    fn dispatch(self: &Arc<Self>, message: Value) -> Result<(), ()> {
        if message.get("method").is_none() {
            let id = message["id"].as_u64().ok_or(())?;
            if self.controls.lock().unwrap().remove(&id) {
                return Ok(());
            }
            let mut pending = self.pending.lock().unwrap();
            let request = pending.get_mut(&id).ok_or(())?;
            let head = request.head.take().ok_or(())?;
            if message.get("error").is_some() {
                request.state.failed.store(true, Ordering::Release);
                request.state.done.store(true, Ordering::Release);
                let _ = head.send(Err("请求失败"));
                pending.remove(&id);
            } else {
                let result = message.get("result").ok_or(())?.clone();
                if head.send(Ok(result)).is_err() {
                    self.cancel(id, &request.state);
                }
            }
            return Ok(());
        }
        if message["method"] != "llmproxy/chunk" {
            // 原生认证刷新或配置警告可能产生通知；服务端请求不能进入工具执行路径。
            return if message.get("id").is_none() && message["method"].is_string() {
                Ok(())
            } else {
                Err(())
            };
        }
        let params = &message["params"];
        let id = params["requestId"].as_u64().ok_or(())?;
        let end = params["end"].as_bool().ok_or(())?;
        let error = params["error"].as_bool().ok_or(())?;
        let data = params["data"].as_str().ok_or(())?;
        if data.len() > 44 * 1024 || (error && !end) || (end && !data.is_empty()) {
            return Err(());
        }
        let mut pending = self.pending.lock().unwrap();
        let request = pending.get_mut(&id).ok_or(())?;
        if request.head.is_some() {
            return Err(());
        }
        if end {
            request.state.failed.fetch_or(error, Ordering::AcqRel);
            request.state.done.store(true, Ordering::Release);
            pending.remove(&id);
        } else if !request.state.cancelled.load(Ordering::Acquire) {
            let bytes = STANDARD.decode(data).map_err(|_| ())?;
            if bytes.len() > 32 * 1024 {
                return Err(());
            }
            if request.body.try_send(Bytes::from(bytes)).is_err() {
                request.state.failed.store(true, Ordering::Release);
                self.cancel(id, &request.state);
            }
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn real_stdio_multiplexes_and_cancels_without_waiting_for_tool_results() {
        let directory =
            std::env::temp_dir().join(format!("llmproxy-native-{}", crate::random_id().unwrap()));
        std::fs::create_dir(&directory).unwrap();
        let program = directory.join("codex");
        std::fs::write(&program, r#"#!/usr/bin/env python3
import sys,json,base64,pathlib
if 'generate-json-schema' in sys.argv:
    out=pathlib.Path(sys.argv[sys.argv.index('--out')+1])
    (out/'llmproxy-proxy-policy.json').write_text(json.dumps({'policyVersion':1,'stateless':True,'clientToolsOnly':True,'methods':['llmproxy/capabilities','llmproxy/responses','llmproxy/cancel']}))
    sys.exit(0)
waiting=[]
def emit(value): print(json.dumps(value),flush=True)
def head(id,kind='text/event-stream'): emit({'id':id,'result':{'status':200,'contentType':kind}})
def chunk(id,text='',end=False,error=False): emit({'method':'llmproxy/chunk','params':{'requestId':id,'data':base64.b64encode(text.encode()).decode(),'end':end,'error':error}})
for line in sys.stdin:
    value=json.loads(line); method=value['method']; id=value.get('id')
    if method=='initialize': emit({'id':id,'result':{}})
    elif method=='initialized': pass
    elif method=='llmproxy/capabilities': emit({'id':id,'result':{'policyVersion':1,'stateless':True,'clientToolsOnly':True,'nativeAuthReady':True}})
    elif method=='llmproxy/cancel':
        chunk(value['params']['requestId'],end=True,error=True)
        emit({'id':id,'result':{}})
    elif method=='llmproxy/responses':
        model=value['params']['request']['model']
        if model in ('one','two'):
            waiting.append((id,model))
            if len(waiting)==2:
                for request,name in reversed(waiting):
                    head(request); chunk(request,name); chunk(request,end=True)
        else:
            head(id,None if model=='missing-type' else 'text/event-stream')
            if model=='missing-type': chunk(id,'finished'); chunk(id,end=True)
            elif model=='overflow':
                for i in range(32): chunk(id,'data')
            elif model=='finish': chunk(id,'finished'); chunk(id,end=True)
            elif model=='broken': chunk(id,'unfinished'); sys.exit(0)
"#).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let native = Native::start(&program).await.unwrap();
        let (one, two) = tokio::join!(
            native.response(json!({"model":"one"})),
            native.response(json!({"model":"two"}))
        );
        assert_eq!(one.unwrap().bytes().await.unwrap(), "one");
        assert_eq!(two.unwrap().bytes().await.unwrap(), "two");
        drop(native.response(json!({"model":"wait"})).await.unwrap());
        for _ in 0..100 {
            if native.core.pending.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(native.core.pending.lock().unwrap().is_empty());
        let mut overflow = native.response(json!({"model":"overflow"})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(overflow.chunk().await.is_err());
        drop(overflow);
        assert_eq!(
            native
                .response(json!({"model":"finish"}))
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap(),
            "finished"
        );
        let missing = native
            .response(json!({"model":"missing-type"}))
            .await
            .unwrap();
        assert!(!missing.headers().contains_key("content-type"));
        assert_eq!(missing.bytes().await.unwrap(), "finished");
        let agent = crate::service::Agent::codex(
            &program,
            1,
            vec!["finish".into()],
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let stop = shutdown.clone();
        let key = "local-test-key".repeat(4);
        let server_key = key.clone();
        let server = tokio::spawn(async move {
            crate::http::serve(agent, address, server_key, stop)
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut health = None;
        for _ in 0..100 {
            if let Ok(response) = client
                .get(format!("http://{address}/health"))
                .bearer_auth(&key)
                .send()
                .await
            {
                health = Some(response.json::<Value>().await.unwrap());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(health.unwrap()["backend"], "codex");
        shutdown.cancel();
        server.await.unwrap();
        match native.response(json!({"model":"broken"})).await {
            Ok(response) => assert!(response.bytes().await.is_err()),
            Err(_) => assert!(native.core.failed.load(Ordering::Acquire)),
        }
        assert!(native.response(json!({"model":"finish"})).await.is_err());
        drop(native);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
