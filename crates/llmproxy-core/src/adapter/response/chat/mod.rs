//! Chat `choices[].message` 与响应 IR。

mod decode;
mod encode;

pub use decode::decode_chat;
pub use encode::encode_chat;

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::OpenAiChat;
