//! Chat Completions 请求消息适配。
//! 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

mod cache;
mod decode;
mod encode;

pub use cache::{decode_chat_cache, encode_chat_cache};
pub use decode::decode_chat;
pub use encode::encode_chat;

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::OpenAiChat;
