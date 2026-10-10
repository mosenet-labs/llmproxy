use std::{convert::Infallible, future::Future, task::Poll, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::{StatusCode, Version, service::service_fn};
use hyper_util::rt::TokioIo;
use llmproxy_console::{Body, Console, RUNTIME_PROTOCOL, Request};
use pingora::{Error, ErrorType, Result, protocols::http::HttpTask, proxy::Session};
use pingora_http::ResponseHeader;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    task::JoinSet,
};

const BUFFER_SIZE: usize = 16 * 1024;

fn failure() -> Box<Error> {
    Error::explain(
        ErrorType::InternalError,
        "console WebSocket transport failed",
    )
}

pub(super) async fn serve(console: &Console, session: &mut Session) -> Result<()> {
    let request = session.req_header();
    let runtime = request
        .headers
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim() == RUNTIME_PROTOCOL);
    if !runtime
        || request.method != "GET"
        || request.version != Version::HTTP_11
        || request.headers.contains_key("transfer-encoding")
        || request
            .headers
            .get("content-length")
            .is_some_and(|v| v != "0")
    {
        session.set_keepalive(None);
        llmproxy_console::observability::transport_failure(Some(400));
        return session.respond_error(400).await;
    }

    // Hyper owns only this bounded in-memory connection, never another listener.
    let (client_io, server_io) = tokio::io::duplex(BUFFER_SIZE);
    let mut tasks = JoinSet::new();
    let auth_console = console.clone();
    let auth_headers = session.req_header().headers.clone();
    let remote = session.client_addr().and_then(|a| a.as_inet()).copied();
    let console = console.clone();
    tasks.spawn(async move {
        let service = service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
            if let Some(address) = remote {
                request
                    .extensions_mut()
                    .insert(llmproxy_console::RemoteAddr(address));
            }
            let console = console.clone();
            async move { Ok::<_, Infallible>(console.handle(request.map(Body::new)).await) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(server_io), service)
            .with_upgrades()
            .await;
    });
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client_io))
        .await
        .map_err(|_| failure())?;
    tasks.spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let request = Request::from_parts(session.req_header().as_owned_parts(), Body::empty());
    let request = request.map(|_| Empty::<Bytes>::new());
    let mut response = tokio::time::timeout(Duration::from_secs(5), sender.send_request(request))
        .await
        .map_err(|_| failure())?
        .map_err(|_| failure())?;
    let upgraded = response.status() == StatusCode::SWITCHING_PROTOCOLS;
    let mut header =
        ResponseHeader::build(response.status().as_u16(), Some(response.headers().len()))?;
    for (name, value) in response.headers() {
        if name != "transfer-encoding" && (upgraded || name != "connection") {
            header.append_header(name, value)?;
        }
    }
    session.set_keepalive(None);
    session
        .write_response_header(Box::new(header), false)
        .await?;
    if !upgraded {
        if response.status().is_client_error() || response.status().is_server_error() {
            llmproxy_console::observability::transport_failure(Some(response.status().as_u16()));
        }
        while let Some(frame) = response.body_mut().frame().await {
            if let Ok(data) = frame.map_err(|_| failure())?.into_data() {
                session.write_response_body(Some(data), false).await?;
            }
        }
        return session.write_response_body(None, true).await;
    }
    let upgraded = hyper::upgrade::on(&mut response)
        .await
        .map_err(|_| failure())?;
    let (revoked, mut revocation) = mpsc::channel(1);
    tasks.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if !auth_console.session_valid(&auth_headers).await {
                let _ = revoked.send(()).await;
                break;
            }
        }
    });
    pump(session, TokioIo::new(upgraded), &mut tasks, &mut revocation).await
}

async fn pump(
    session: &mut Session,
    io: TokioIo<hyper::upgrade::Upgraded>,
    tasks: &mut JoinSet<()>,
    revocation: &mut mpsc::Receiver<()>,
) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(io);
    let (to_server, mut client_bytes) = mpsc::channel::<Bytes>(1);
    let (to_client, mut server_bytes) = mpsc::channel(1);
    tasks.spawn(async move {
        while let Some(bytes) = client_bytes.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                return;
            }
        }
        let _ = writer.shutdown().await;
    });
    tasks.spawn(async move {
        let mut buffer = [0; BUFFER_SIZE];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    if to_client
                        .send(Ok(Bytes::copy_from_slice(&buffer[..n])))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => {
                    let _ = to_client.send(Err(failure())).await;
                    break;
                }
            }
        }
    });
    session.set_proxy_tasks_enabled(true);
    let mut permit = None;
    let mut reserve = Box::pin(to_server.clone().reserve_owned());
    let mut server_closed = false;
    let mut shutdown = tokio::time::interval(Duration::from_secs(1));
    // Read and queued writes share Pingora's Session. Its upgraded reader and
    // proxy-task writer retain their progress when these futures are re-polled.
    std::future::poll_fn(|cx| {
        if revocation.poll_recv(cx).is_ready() {
            return Poll::Ready(Ok(()));
        }
        if shutdown.poll_tick(cx).is_ready() && session.is_process_shutting_down() {
            return Poll::Ready(Ok(()));
        }
        if session.has_pending_downstream_tasks() {
            match std::pin::pin!(session.write_downstream_proxy_tasks()).poll(cx) {
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(_)) if server_closed => return Poll::Ready(Ok(())),
                _ => {}
            }
        }
        if permit.is_none() {
            match reserve.as_mut().poll(cx) {
                Poll::Ready(Ok(ready)) => permit = Some(ready),
                Poll::Ready(Err(_)) => return Poll::Ready(Err(failure())),
                Poll::Pending => {}
            }
        }
        if permit.is_some() {
            match std::pin::pin!(session.read_request_body()).poll(cx) {
                Poll::Ready(Ok(Some(bytes))) => {
                    permit.take().expect("reserved upstream slot").send(bytes);
                    reserve = Box::pin(to_server.clone().reserve_owned());
                    cx.waker().wake_by_ref();
                }
                Poll::Ready(Ok(None)) => return Poll::Ready(Ok(())),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {}
            }
        }
        if !session.has_pending_downstream_tasks() && !server_closed {
            match server_bytes.poll_recv(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    session
                        .downstream_session
                        .send_downstream_proxy_task(HttpTask::UpgradedBody(Some(bytes), false));
                    cx.waker().wake_by_ref();
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => server_closed = true,
                Poll::Pending => {}
            }
        }
        if server_closed && !session.has_pending_downstream_tasks() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    })
    .await
}
