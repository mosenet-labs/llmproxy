use crate::{
    Error,
    service::{Agent, BODY_LIMIT, Reply},
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::{
    Request, Response,
    body::{Body, Frame, Incoming},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub async fn limited_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "HTTP 响应读取失败")? {
        if bytes.len() + chunk.len() > limit {
            return Err("HTTP 响应超过容量上限".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub async fn serve(
    agent: Arc<Agent>,
    listen: SocketAddr,
    key: String,
    shutdown: CancellationToken,
) -> Result<(), Error> {
    if key.len() < 32 {
        return Err("本地 HTTP 访问密钥至少 32 字符".into());
    }
    let listener = TcpListener::bind(listen).await?;
    println!(
        "Responses HTTP：{}，backend={}",
        listener.local_addr()?,
        agent.backend_kind()
    );
    loop {
        tokio::select! {
            _=shutdown.cancelled()=>return Ok(()),
            socket=listener.accept()=> {
                let (socket,_)=socket?;
                let agent=agent.clone(); let key=key.clone(); let shutdown=shutdown.clone();
                tokio::spawn(async move {
                    let service=service_fn(move |request| handle(agent.clone(),key.clone(),request));
                    let connection=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket),service);
                    tokio::select! { _=shutdown.cancelled()=>{}, _=connection=>{} }
                });
            }
        }
    }
}

type HttpBody = UnsyncBoxBody<Bytes, Infallible>;

async fn handle(
    agent: Arc<Agent>,
    key: String,
    mut request: Request<Incoming>,
) -> Result<Response<HttpBody>, Infallible> {
    let supplied = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !secret_eq(supplied, &key) {
        return Ok(reply(Reply::error(401, "unauthorized", "访问凭据无效")));
    }
    if request.method() == "GET" && request.uri().path() == "/health" {
        let body = serde_json::json!({"backend":agent.backend_kind(),"backend_status":agent.health(),"concurrency":agent.concurrency});
        return Ok(Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body.to_string())).boxed_unsync())
            .unwrap());
    }
    if request.method() == "GET" && request.uri().path() == "/v1/models" {
        let data: Vec<_> = agent
            .models
            .iter()
            .map(|id| serde_json::json!({"id":id,"object":"model","owned_by":"subscription-agent"}))
            .collect();
        return Ok(Response::builder()
            .header("content-type", "application/json")
            .body(
                Full::new(Bytes::from(
                    serde_json::json!({"object":"list","data":data}).to_string(),
                ))
                .boxed_unsync(),
            )
            .unwrap());
    }
    if request.uri().path() != "/v1/responses" {
        return Ok(reply(Reply::error(404, "not_found", "未知路径")));
    }
    if request.method() != "POST" {
        return Ok(reply(Reply::error(
            405,
            "method_not_allowed",
            "需要 POST 请求",
        )));
    }
    let read = tokio::time::timeout(Duration::from_secs(30), async {
        let mut bytes = Vec::new();
        while let Some(frame) = request.body_mut().frame().await {
            let frame = frame.map_err(|_| ())?;
            if let Ok(chunk) = frame.into_data() {
                if bytes.len() + chunk.len() > BODY_LIMIT {
                    return Err(());
                }
                bytes.extend_from_slice(&chunk);
            }
        }
        Ok::<_, ()>(bytes)
    })
    .await;
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return Ok(reply(Reply::error(
                413,
                "invalid_body",
                "请求截断或超过容量上限",
            )));
        }
        Err(_) => return Ok(reply(Reply::error(408, "timeout", "请求读取超时"))),
    };
    let cancel = CancellationToken::new();
    let response = agent.respond(&bytes, cancel).await;
    Ok(reply(response))
}

pub fn secret_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn reply(response: Reply) -> Response<HttpBody> {
    Response::builder()
        .status(response.status)
        .header("content-type", response.content_type)
        .header("cache-control", "no-store")
        .body(ReplyBody(response).boxed_unsync())
        .unwrap()
}

struct ReplyBody(Reply);
impl Body for ReplyBody {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0
            .body
            .poll_recv(cx)
            .map(|bytes| bytes.map(|bytes| Ok(Frame::data(bytes))))
    }
}
impl Drop for ReplyBody {
    fn drop(&mut self) {
        self.0.cancel.cancel();
    }
}
