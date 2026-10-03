//! 请求消息序列适配器；调用方负责 HTTP 正文的读取和重写。

pub(in crate::adapter) mod chat;
mod gemini;
mod messages;
mod responses;

pub use super::Error;
use super::{Result, wire};

#[cfg(test)]
mod tests;

pub use chat::{decode_chat, decode_chat_cache, encode_chat, encode_chat_cache};
pub use gemini::{decode_gemini, decode_gemini_cache, encode_gemini, encode_gemini_cache};
pub use messages::{
    decode_messages, decode_messages_cache, encode_messages, encode_messages_cache,
};
pub use responses::{
    decode_responses, decode_responses_cache, encode_responses, encode_responses_cache,
};
