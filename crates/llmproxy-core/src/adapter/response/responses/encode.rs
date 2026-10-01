//! 复用可回传 OutputMessage 的请求侧编码规则。

use crate::{
    ir::response::Message as IrMessage,
    protocol::{
        Protocol,
        responses::{request::message::Message as InputItem, response::message::Message},
    },
};

use super::super::{Error, Result, reject_unmapped_chat, reject_unmapped_parts};

/// 需要来源消息带有输出项必需的 ID、状态和类型字段。
pub fn encode_responses(messages: &[IrMessage]) -> Result<Vec<Message>> {
    for message in messages {
        reject_unmapped_chat(message, Protocol::OpenAiResponses)?;
        reject_unmapped_parts(message, Protocol::OpenAiResponses)?;
    }
    let messages = messages.iter().cloned().map(Into::into).collect::<Vec<_>>();
    crate::adapter::request::encode_responses(&messages)?
        .into_iter()
        .map(|item| match item {
            InputItem::Output(message) => Ok(message),
            _ => Err(Error::Unsupported(
                "缺少 Responses 输出消息的 ID、状态或类型".into(),
            )),
        })
        .collect()
}
