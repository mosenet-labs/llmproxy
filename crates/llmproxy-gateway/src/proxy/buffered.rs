//! 预先准备跨协议请求正文；父请求负责整包或逐帧响应的最终交付。
use super::{Gateway, RequestContext};
use bytes::Bytes;
use pingora::{
    Error, ErrorType, Result,
    protocols::http::HttpTask,
    proxy::{
        ProxyHttp, Session,
        subrequest::{BodyMode, Ctx},
    },
};
use pingora_http::ResponseHeader;
use std::sync::{Arc, Mutex};

/// 同一份上下文在父子请求间移动，固定路由快照并避免重复记录遥测。
#[derive(Clone)]
struct Exchange(Arc<Mutex<Option<RequestContext>>>);

/// 仅接受框架私有上下文标记，客户端请求头不能绕过路由和鉴权。
fn exchange(session: &Session) -> Option<&Exchange> {
    session.subrequest_ctx.as_ref()?.user_ctx()?.downcast_ref()
}

/// 子请求接管已选定的 Provider 和请求前缀，不再次选路或预读模型。
pub(super) fn resume(session: &Session, ctx: &mut RequestContext) -> Result<bool> {
    let Some(exchange) = exchange(session) else {
        return Ok(false);
    };
    *ctx = exchange
        .0
        .lock()
        .expect("subrequest context mutex")
        .take()
        .ok_or_else(|| {
            Error::explain(
                ErrorType::InternalError,
                "subrequest context already consumed",
            )
        })?;
    Ok(true)
}

/// 子请求结束时归还遥测上下文，由父请求按最终客户端状态完成一次统计。
pub(super) fn complete(gateway: &Gateway, session: &Session, ctx: &mut RequestContext) -> bool {
    let Some(exchange) = exchange(session) else {
        return false;
    };
    *exchange.0.lock().expect("subrequest context mutex") =
        Some(std::mem::replace(ctx, gateway.new_ctx()));
    true
}

/// 成功返回 true 表示响应已完成；同协议继续原有流式／透传路径。
pub(super) async fn forward(
    gateway: &Gateway,
    session: &mut Session,
    ctx: &mut RequestContext,
) -> Result<bool> {
    if !ctx
        .route
        .as_ref()
        .expect("request_filter selected route")
        .is_cross_protocol()
        && ctx.thinking == llmproxy_core::thinking::Choice::Default
    {
        return Ok(false);
    }
    let input = ctx.request_body.buffered_input(session).await?;
    let mut request = ctx
        .request_body
        .decode_cross_request(&input, ctx.route.as_ref().unwrap().stream)?;
    ctx.route.as_mut().unwrap().stream = request.generation.stream;
    super::thinking::apply(ctx, &mut request)?;
    if let Some(history) = &ctx.history {
        history.sink.reasoning(&request.generation.reasoning);
    }
    // 签名恢复可能等待持久化存储，准备期间也监视客户端断开并取消该 future。
    tokio::select! {
        prepared = ctx.telemetry.instrument(ctx.request_body.prepare_cross_request(&request)) => prepared?,
        closed = session.read_body_or_idle(true) => return Err(closed.err().unwrap_or_else(|| {
            Error::explain(ErrorType::ConnectionClosed, "client closed during request preparation")
        }).into_down()),
    }
    // 子请求只需要目标字节与原始输入边界，不持有整份来源结构体和 IR。
    drop(request);
    let spawner = session.subrequest_spawner.as_ref().ok_or_else(|| {
        Error::explain(ErrorType::InternalError, "subrequest spawner unavailable")
    })?;
    let protocol = ctx.protocol;
    let mut streaming = if ctx.route.as_ref().unwrap().stream {
        Some(
            ctx.telemetry
                .in_scope(|| super::streaming::Transfer::new(ctx))?,
        )
    } else {
        None
    };
    let span = ctx.telemetry.in_scope(tracing::Span::current);
    let telemetry = ctx.telemetry.cancellation_snapshot();
    let route = ctx.route.clone();
    let history = ctx.history.clone();
    let shared = Exchange(Arc::new(Mutex::new(Some(std::mem::replace(
        ctx,
        gateway.new_ctx(),
    )))));
    ctx.protocol = protocol;
    ctx.telemetry = telemetry;
    ctx.route = route;
    ctx.history = history;
    let (request, handle) = spawner.create_subrequest(
        session.as_downstream(),
        Ctx::builder()
            .body_mode(BodyMode::ExpectBody)
            .user_ctx(Box::new(shared.clone()))
            .build(),
    );
    let mut error_rx = handle.subreq_proxy_error;
    let mut rx = handle.rx;
    // 发送完正文后仍持有发送端；提前关闭会被子请求当作客户端断开。
    let tx = handle.tx;
    if let Some(transfer) = &mut streaming {
        use tracing::Instrument;
        // 接收或发送失败都会取消其余 future，释放上游，不创建游离任务。
        let outcome = tokio::try_join!(
            async {
                request.run().await;
                // 子请求失败通知可能先于已入队正文被父请求读取；先按序交付，避免丢掉有效增量。
                Ok::<_, Box<Error>>(error_rx.try_recv().ok())
            },
            async {
                for chunk in input {
                    tx.send(HttpTask::Body(Some(chunk), false))
                        .await
                        .map_err(|_| {
                            Error::explain(ErrorType::ConnectionClosed, "subrequest input closed")
                        })?;
                }
                tx.send(HttpTask::Body(None, true)).await.map_err(|_| {
                    Error::explain(ErrorType::ConnectionClosed, "subrequest input closed")
                })?;
                Ok(())
            },
            transfer.run(session, &mut rx).instrument(span)
        )
        .and_then(|(error, _, _)| error.map_or(Ok(()), Err));
        if let Some(restored) = shared.0.lock().expect("subrequest context mutex").take() {
            *ctx = restored;
        } else if let Some(status) = transfer.upstream_status() {
            ctx.telemetry.response_headers(status);
        }
        if let Err(error) = &outcome {
            ctx.stream_error = transfer.failure(super::error_status(error));
        }
        outcome?;
        return Ok(true);
    }
    // 框架的 idle 读取监视客户端关闭；子请求执行期间父请求没有写出响应头。
    // 任一关闭事件会销毁整个子请求 future，从而主动释放上游连接。
    let outcome = tokio::select! {
        completed = async { tokio::join!(
        request.run(),
        async {
            for chunk in input {
                if tx.send(HttpTask::Body(Some(chunk), false)).await.is_err() {
                    return;
                }
            }
            let _ = tx.send(HttpTask::Body(None, true)).await;
        },
        async move {
            let mut response = Captured::default();
            while let Some(task) = rx.recv().await {
                response.push(task)?;
            }
            response.finish()
        }
        ) } => Ok(completed),
        closed = session.read_body_or_idle(true) => Err(closed.err().unwrap_or_else(|| {
            Error::explain(ErrorType::ConnectionClosed, "client closed during buffered response")
        }).into_down()),
    };
    if let Some(restored) = shared.0.lock().expect("subrequest context mutex").take() {
        *ctx = restored;
    }
    let (_, _, captured) = outcome?;
    // 通道关闭与失败通知可能同一次轮询就绪，等子请求结束后优先核对错误。
    if let Ok(error) = error_rx.try_recv() {
        if error.esource() == &pingora::ErrorSource::Downstream {
            captured?;
        }
        return Err(error);
    }
    let (mut header, body) = captured?;
    // 新工具引用必须先持久化，再提交真实响应头；断开时一起取消存储 future。
    tokio::select! {
        persisted = ctx.telemetry.instrument(ctx.response_body.persist_tool_state()) => persisted?,
        closed = session.read_body_or_idle(true) => return Err(closed.err().unwrap_or_else(|| {
            Error::explain(ErrorType::ConnectionClosed, "client closed during tool state commit")
        }).into_down()),
    }
    header.remove_header("transfer-encoding");
    header.remove_header("content-length");
    header.remove_header("etag");
    header.insert_header("content-length", body.len().to_string())?;
    session
        .write_response_header(header, body.is_empty())
        .await?;
    if !body.is_empty() {
        session.write_response_body(Some(body), true).await?;
    }
    Ok(true)
}

/// 只缓冲已转换后的输出；响应头和结束标记也必须完整收到。
#[derive(Default)]
pub(super) struct Captured {
    header: Option<Box<ResponseHeader>>,
    body: Vec<u8>,
    ended: bool,
}
impl Captured {
    /// 不向客户端发送任何 HttpTask；错误立即终止采集。
    pub(super) fn push(&mut self, task: HttpTask) -> Result<()> {
        match task {
            HttpTask::Header(header, end) if !header.status.is_informational() => {
                self.header = Some(header);
                self.ended |= end;
            }
            HttpTask::Header(_, _) => {}
            HttpTask::Body(body, end) => {
                if let Some(body) = body {
                    if self.body.len().saturating_add(body.len())
                        > crate::transform::MAX_BUFFERED_BODY
                    {
                        return Err(Error::explain(
                            ErrorType::HTTPStatus(502),
                            "converted response exceeds limit",
                        ));
                    }
                    self.body.extend_from_slice(&body);
                }
                self.ended |= end;
            }
            HttpTask::Done | HttpTask::Trailer(_) => self.ended = true,
            HttpTask::Failed(error) => return Err(error),
            HttpTask::UpgradedBody(_, _) => {
                return Err(Error::explain(
                    ErrorType::HTTPStatus(502),
                    "unexpected upgraded response",
                ));
            }
        }
        Ok(())
    }
    /// 已收响应头不等于成功，缺失正文结束标记时必须返回网关错误。
    pub(super) fn finish(self) -> Result<(Box<ResponseHeader>, Bytes)> {
        match (self.header, self.ended) {
            (Some(header), true) => Ok((header, Bytes::from(self.body))),
            _ => Err(Error::explain(
                ErrorType::HTTPStatus(502),
                "incomplete converted response",
            )),
        }
    }
}
