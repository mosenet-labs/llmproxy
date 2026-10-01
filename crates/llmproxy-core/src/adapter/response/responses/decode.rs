//! 复用可回传 OutputMessage 的请求侧解码规则。

use crate::{
    ir::response::Message as IrMessage,
    protocol::responses::{request::message::Message as InputItem, response::message::Message},
};

use super::super::Result;

/// 仅处理 `type: "message"` 的输出项。
pub fn decode_responses(messages: &[Message]) -> Result<Vec<IrMessage>> {
    let items = messages
        .iter()
        .cloned()
        .map(InputItem::Output)
        .collect::<Vec<_>>();
    crate::adapter::request::decode_responses(&items)
        .map(|messages| messages.into_iter().map(Into::into).collect())
}
