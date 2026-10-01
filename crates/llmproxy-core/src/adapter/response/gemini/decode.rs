//! 复用同形 Content 的请求侧解码规则。

use crate::{ir::response::Message as IrMessage, protocol::gemini::response::message::Message};

use super::super::Result;

/// 保留候选消息的片段顺序、思考签名和扩展字段。
pub fn decode_gemini(messages: &[Message]) -> Result<Vec<IrMessage>> {
    crate::adapter::request::decode_gemini(messages)
        .map(|messages| messages.into_iter().map(Into::into).collect())
}
