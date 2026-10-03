//! 四协议请求结构体的统一载体。

use serde::{Deserialize, Serialize};

use crate::protocol::{Protocol, chat, gemini, messages, responses};

/// 编解码器的类型化请求；既用于来源输入，也用于目标输出。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Chat(Box<chat::request::Request>),
    Responses(Box<responses::request::Request>),
    Messages(Box<messages::request::Request>),
    Gemini(Box<gemini::request::Request>),
}

impl Request {
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
