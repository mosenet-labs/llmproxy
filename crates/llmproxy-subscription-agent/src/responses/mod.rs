//! 共用 Responses 类型；订阅上游的请求策略及非流式终态收集。

mod collect;
mod request;

pub use collect::Collector;
pub use request::{
    Prepared, RequestError, prepare_codex_request, prepare_oauth_request, validate_stateless,
};
