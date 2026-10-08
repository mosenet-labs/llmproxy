//! Model discovery uses the same snapshot resolver as inference requests.
use bytes::Bytes;
use pingora::{Error, ErrorType, Result, proxy::Session};
use pingora_http::ResponseHeader;

use crate::snapshot::ProviderSnapshots;

pub(super) fn matches(path: &str) -> bool {
    matches!(path, "/models" | "/v1/models")
}

pub(super) async fn serve(providers: &ProviderSnapshots, session: &mut Session) -> Result<()> {
    if session.req_header().method != "GET" {
        session.set_keepalive(None);
        let mut header = ResponseHeader::build(405, Some(2))?;
        header.insert_header("allow", "GET")?;
        header.insert_header("content-length", "0")?;
        session
            .write_response_header(Box::new(header), true)
            .await?;
        return Ok(());
    }
    let body = serde_json::to_vec(&serde_json::json!({
        "object": "list", "data": providers.catalog(),
    }))
    .map_err(|_| Error::explain(ErrorType::InternalError, "cannot serialize model catalog"))?;
    // A discovery request does not consume a body; do not reuse an unread connection.
    session.set_keepalive(None);
    let mut header = ResponseHeader::build(200, Some(3))?;
    header.insert_header("content-type", "application/json")?;
    header.insert_header("content-length", body.len().to_string())?;
    header.insert_header("cache-control", "no-store")?;
    session
        .write_response_header(Box::new(header), false)
        .await?;
    session
        .write_response_body(Some(Bytes::from(body)), true)
        .await?;
    Ok(())
}
