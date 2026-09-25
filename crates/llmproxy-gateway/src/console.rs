//! In-process transport adapter; Pingora owns the only listening socket.
use std::time::Duration;

use http_body_util::BodyExt;
use llmproxy_console::{BODY_LIMIT, Body, Console, Request};
use pingora::{Error, ErrorType, Result, proxy::Session};
use pingora_http::ResponseHeader;

pub fn matches(path: &str) -> bool {
    path == "/ui" || path.starts_with("/ui/")
}

pub async fn serve(console: &Console, session: &mut Session) -> Result<()> {
    // Preserve the console's local-only access even on a public proxy listener.
    let local = session
        .client_addr()
        .and_then(|address| address.as_inet())
        .is_some_and(|address| address.ip().is_loopback());
    if !local {
        session.set_keepalive(None);
        session.respond_error(403).await?;
        llmproxy_console::observability::transport_failure(Some(403));
        return Ok(());
    }
    let parts = session.req_header().as_owned_parts();
    let head = parts.method == "HEAD";
    let body = tokio::time::timeout(Duration::from_secs(30), read_body(session))
        .await
        .map_err(|_| {
            Error::explain(ErrorType::HTTPStatus(408), "console request body timed out").into_down()
        })??;
    let response = console
        .handle(Request::from_parts(parts, Body::from(body)))
        .await;
    let (parts, mut body) = response.into_parts();
    let mut header = ResponseHeader::build(parts.status.as_u16(), Some(parts.headers.len()))?;
    for (name, value) in &parts.headers {
        // Downstream transfer framing belongs to Pingora.
        if name != "transfer-encoding" && name != "connection" {
            header.append_header(name, value)?;
        }
    }
    let empty = head || parts.status.as_u16() == 204 || parts.status.as_u16() == 304;
    session
        .write_response_header(Box::new(header), empty)
        .await?;
    if !empty {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| {
                Error::explain(ErrorType::InternalError, "console response body failed")
            })?;
            match frame.into_data() {
                Ok(data) => session.write_response_body(Some(data), false).await?,
                Err(frame) => {
                    if let Ok(trailers) = frame.into_trailers() {
                        session.write_response_trailers(trailers).await?;
                    }
                }
            }
        }
        session.write_response_body(None, true).await?;
    }
    Ok(())
}

async fn read_body(session: &mut Session) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = session.read_request_body().await? {
        // One extra byte lets Topcoat's native BodyLimit return its 413 response.
        let remaining = BODY_LIMIT + 1 - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() > BODY_LIMIT {
            // Never reuse an HTTP/1 connection with an unread oversized body.
            session.set_keepalive(None);
            break;
        }
    }
    Ok(body)
}
