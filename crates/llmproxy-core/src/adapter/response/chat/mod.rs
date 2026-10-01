//! Chat `choices[].message` 与响应 IR。

mod decode;
mod encode;
mod usage;

pub use decode::decode_chat;
pub use encode::encode_chat;
pub use usage::{decode_chat_usage, encode_chat_usage};

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::OpenAiChat;
