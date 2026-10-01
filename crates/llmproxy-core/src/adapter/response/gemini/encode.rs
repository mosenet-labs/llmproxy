//! 复用同形 Content 的请求侧编码规则。

use crate::protocol::Protocol;
use crate::{ir::response::Message as IrMessage, protocol::gemini::response::message::Message};

use super::super::{Result, reject_unmapped_chat, reject_unmapped_parts};

/// 将 IR 消息编码为 Gemini 候选 Content。
pub fn encode_gemini(messages: &[IrMessage]) -> Result<Vec<Message>> {
    for message in messages {
        reject_unmapped_chat(message, Protocol::Gemini)?;
        reject_unmapped_parts(message, Protocol::Gemini)?;
    }
    let messages = messages.iter().cloned().map(Into::into).collect::<Vec<_>>();
    crate::adapter::request::encode_gemini(&messages)
}
