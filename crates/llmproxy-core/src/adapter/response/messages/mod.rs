//! Anthropic 顶层响应 Message 与响应 IR。

mod decode;
mod encode;

pub use decode::decode_messages;
pub use encode::encode_messages;

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::AnthropicMessages;
