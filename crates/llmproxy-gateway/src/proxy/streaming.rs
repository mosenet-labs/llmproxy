//! 父请求逐帧交付跨协议 SSE；异步发送自然阻塞接收，背压传回子请求和上游。
use super::{RequestContext, buffered::Captured};
use crate::transform::stream::Stream;
use bytes::Bytes;
use llmproxy_core::adapter::protocol_codec::ResponseTarget;
use pingora::{Error, ErrorType, Result, protocols::http::HttpTask, proxy::Session};
use pingora_http::ResponseHeader;
use tokio::sync::mpsc::Receiver;

pub(super) struct Transfer {
    stream: Stream,
    header: Option<Box<ResponseHeader>>,
    error: Option<Captured>,
    ended: bool,
    upstream_status: Option<u16>,
    write_timeout: std::time::Duration,
}
impl Transfer {
    /// 在上下文移交子请求前固定目标外壳、来源协议和签名作用域。
    pub(super) fn new(ctx: &RequestContext) -> Result<Self> {
        Ok(Self {
            stream: Stream::new(
                ctx.provider.as_ref().unwrap().protocol,
                ctx.protocol.unwrap(),
                &ResponseTarget {
                    model: ctx.client_model.as_deref().unwrap(),
                    id: &ctx.response_id,
                    created: ctx.response_created,
                },
                ctx.response_body.tool_context(),
            )?,
            header: None,
            error: None,
            ended: false,
            upstream_status: None,
            write_timeout: std::time::Duration::from_millis(
                ctx.provider.as_ref().unwrap().write_timeout_ms,
            ),
        })
    }
    /// 收到完整帧就转换发送，正文后续尚未就绪时监视真实客户端关闭。
    pub(super) async fn run(
        &mut self,
        session: &mut Session,
        rx: &mut Receiver<HttpTask>,
    ) -> Result<()> {
        let result = self.receive(session, rx).await;
        self.stream.record_usage();
        result
    }
    /// 顺序消费子请求任务；异常在最后有效帧之后交给父请求失败回调。
    async fn receive(&mut self, session: &mut Session, rx: &mut Receiver<HttpTask>) -> Result<()> {
        loop {
            let task = tokio::select! {
                task = rx.recv() => task,
                closed = session.read_body_or_idle(true) => return Err(closed.err().unwrap_or_else(|| Error::explain(ErrorType::ConnectionClosed, "client closed during stream")).into_down()),
            };
            let Some(task) = task else {
                return if self.ended {
                    Ok(())
                } else {
                    Err(incomplete())
                };
            };
            if let Some(error) = &mut self.error {
                error.push(task)?;
                continue;
            }
            match task {
                HttpTask::Header(header, end) if !header.status.is_informational() => {
                    self.upstream_status = Some(header.status.as_u16());
                    if self.header.is_some() || session.response_written().is_some() {
                        return Err(incomplete());
                    }
                    if !header.status.is_success() {
                        let mut capture = Captured::default();
                        capture.push(HttpTask::Header(header, end))?;
                        self.error = Some(capture);
                    } else {
                        self.header = Some(header);
                        if end {
                            self.finish(session).await?;
                        }
                    }
                }
                HttpTask::Header(_, _) => {}
                HttpTask::Body(body, end) => {
                    if self.ended {
                        if body.as_ref().is_some_and(|b| !b.is_empty()) {
                            return Err(incomplete());
                        }
                        continue;
                    }
                    if self.header.is_none() && session.response_written().is_none() {
                        return Err(incomplete());
                    }
                    if let Some(body) = body {
                        for byte in body {
                            if let Some(frame) = self.stream.frame(byte)? {
                                let output = tokio::select! {
                                    output = self.stream.convert(&frame) => output?,
                                    closed = session.read_body_or_idle(true) => return Err(closed.err().unwrap_or_else(incomplete).into_down()),
                                };
                                self.write(session, output).await?;
                            }
                        }
                    }
                    if end {
                        self.finish(session).await?;
                    }
                }
                HttpTask::Done | HttpTask::Trailer(_) => {
                    if !self.ended {
                        self.finish(session).await?;
                    }
                }
                HttpTask::Failed(error) => return Err(error),
                HttpTask::UpgradedBody(_, _) => return Err(incomplete()),
            }
            // 非成功 HTTP 响应仍先完整转换错误正文再提交，不能写成流内成功事件。
            if self.error.is_some() {
                break;
            }
        }
        while let Some(task) = tokio::select! { task = rx.recv() => task, closed = session.read_body_or_idle(true) => return Err(closed.err().unwrap_or_else(incomplete).into_down()) }
        {
            self.error.as_mut().unwrap().push(task)?;
        }
        let (mut header, body) = self.error.take().unwrap().finish()?;
        header.remove_header("transfer-encoding");
        header.insert_header("content-length", body.len().to_string())?;
        session.write_response_header(header, false).await?;
        session.write_response_body(Some(body), true).await
    }
    /// 先提交待发头，再顺序 await 每帧，应用层没有无界输出队列。
    async fn write(&mut self, session: &mut Session, output: Vec<Bytes>) -> Result<()> {
        if output.is_empty() {
            return Ok(());
        }
        if let Some(mut header) = self.header.take() {
            header.remove_header("content-length");
            header.remove_header("transfer-encoding");
            session.write_response_header(header, false).await?;
        }
        for body in output {
            tokio::time::timeout(
                self.write_timeout,
                session.write_response_body(Some(body), false),
            )
            .await
            .map_err(|_| {
                Error::explain(ErrorType::WriteTimedout, "client stream write timed out")
                    .into_down()
            })??;
        }
        Ok(())
    }
    /// HTTP 结束后再交付协议结束事件，不以合法帧之前的断开视为成功。
    async fn finish(&mut self, session: &mut Session) -> Result<()> {
        let output = self.stream.finish().await?;
        self.write(session, output).await?;
        if self.header.is_some() {
            return Err(incomplete());
        }
        session.write_response_body(None, true).await?;
        self.ended = true;
        Ok(())
    }
    /// 返回安全且顺序正确的客户端流内错误事件，由 Gateway 最终失败回调发送。
    pub(super) fn failure(&self, status: u16) -> Bytes {
        self.stream.failure(status)
    }
    /// 子请求被取消时仍能判定失败发生在响应正文阶段，不制造连接时间数据。
    pub(super) fn upstream_status(&self) -> Option<u16> {
        self.upstream_status
    }
}
/// 统一使用安全网关错误，来源帧内容不会进入错误或日志。
fn incomplete() -> Box<Error> {
    Error::explain(ErrorType::HTTPStatus(502), "incomplete upstream stream").into_up()
}
