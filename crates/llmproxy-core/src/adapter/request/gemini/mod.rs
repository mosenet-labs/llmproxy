//! Gemini generateContent 请求消息适配。
//! 参考：https://ai.google.dev/api/generate-content

mod decode;
mod encode;

pub use decode::decode_gemini;
pub use encode::encode_gemini;

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::Gemini;
