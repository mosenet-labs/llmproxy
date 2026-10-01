//! 非流式响应消息序列适配器；外层响应对象和 SSE 事件由调用方处理。

mod chat;
mod gemini;
mod messages;
mod responses;

#[cfg(test)]
mod tests;

pub use super::Error;
use super::{Result, wire};

use crate::{ir::response::Message as IrMessage, protocol::Protocol};
use serde_json::Value;

fn has_content(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(value) => *value,
        _ => true,
    }
}

fn reject_unmapped_chat(message: &IrMessage, target: Protocol) -> Result<()> {
    if target == Protocol::OpenAiChat {
        return Ok(());
    }
    let extra = wire::extra(&message.metadata, Protocol::OpenAiChat);
    for key in ["audio", "function_call", "annotations"] {
        if extra.get(key).is_some_and(has_content) {
            return Err(Error::Unsupported(format!(
                "Chat 响应字段 {key} 尚不能跨协议转换"
            )));
        }
    }
    Ok(())
}

fn reject_unmapped_parts(message: &IrMessage, target: Protocol) -> Result<()> {
    for part in &message.parts {
        for (source, fields) in [
            (Protocol::OpenAiResponses, &["annotations", "logprobs"][..]),
            (Protocol::AnthropicMessages, &["citations"][..]),
            (
                Protocol::Gemini,
                &[
                    "thoughtSignature",
                    "thought",
                    "partMetadata",
                    "speechMetadata",
                ][..],
            ),
        ] {
            if source == target {
                continue;
            }
            let extra = wire::extra(&part.metadata, source);
            for field in fields {
                if extra.get(*field).is_some_and(has_content) {
                    return Err(Error::Unsupported(format!(
                        "来源协议的 {field} 内容无法写入目标响应消息"
                    )));
                }
            }
        }
    }
    Ok(())
}

pub use chat::{decode_chat, encode_chat};
pub use gemini::{decode_gemini, encode_gemini};
pub use messages::{decode_messages, encode_messages};
pub use responses::{decode_responses, encode_responses};
