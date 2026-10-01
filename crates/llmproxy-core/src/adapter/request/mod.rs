//! 请求消息序列适配器；调用方负责 HTTP 正文的读取和重写。

mod chat;
mod gemini;
mod messages;
mod responses;

pub use super::Error;
use super::{Result, wire};

#[cfg(test)]
mod tests;

pub use chat::{decode_chat, encode_chat};
pub use gemini::{decode_gemini, encode_gemini};
pub use messages::{decode_messages, encode_messages};
pub use responses::{decode_responses, encode_responses};
