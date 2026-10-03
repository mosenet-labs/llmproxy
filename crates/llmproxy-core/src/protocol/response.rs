//! 四协议非流式响应结构体的统一载体。

use serde::{Deserialize, Serialize};

use crate::protocol::{Protocol, chat, gemini, messages, responses};

/// 编解码器的类型化响应；既用于来源输入，也用于目标输出。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Chat(Box<chat::response::Completion>),
    Responses(Box<responses::response::Response>),
    Messages(Box<messages::response::Message>),
    Gemini(Box<gemini::response::Response>),
}

impl Response {
    /// 返回当前载体对应的协议。
    pub fn protocol(&self) -> Protocol {
        match self {
            Self::Chat(_) => Protocol::OpenAiChat,
            Self::Responses(_) => Protocol::OpenAiResponses,
            Self::Messages(_) => Protocol::AnthropicMessages,
            Self::Gemini(_) => Protocol::Gemini,
        }
    }
}
