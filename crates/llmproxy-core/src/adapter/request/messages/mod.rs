//! Anthropic Messages 请求消息适配。
//! 参考：https://platform.claude.com/docs/en/api/http/messages

mod decode;
mod encode;

pub use decode::decode_messages;
pub use encode::encode_messages;

use crate::protocol::Protocol;

const PROTOCOL: Protocol = Protocol::AnthropicMessages;
