//! Responses 请求 `input` 中消息项的适配，不包含独立工具项。
//! 参考：https://developers.openai.com/api/reference/resources/responses/methods/create

mod cache;
mod decode;
mod encode;

pub use cache::{decode_responses_cache, encode_responses_cache};

pub use decode::decode_responses;
pub use encode::encode_responses;

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::OpenAiResponses;
