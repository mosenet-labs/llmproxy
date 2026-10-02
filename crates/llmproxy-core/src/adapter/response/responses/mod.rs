//! Responses `output[]` 中的消息项与响应 IR。

mod decode;
mod encode;
mod usage;

pub use usage::{decode_responses_usage, encode_responses_usage};

pub use decode::decode_responses;
pub use encode::encode_responses;
