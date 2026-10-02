//! 同协议回写所需的完整非流式响应类型。

use serde::{Deserialize, Serialize};

use crate::protocol::{Protocol, chat, gemini, messages, responses};

/// 来源响应的协议类型，保留尚未映射到通用 IR 的字段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Source {
    Chat(Box<chat::response::Completion>),
    Responses(Box<responses::response::Response>),
    Messages(Box<messages::response::Message>),
    Gemini(Box<gemini::response::Response>),
}

impl Source {
    /// 返回来源协议。
    pub(crate) fn protocol(&self) -> Protocol {
        match self {
            Self::Chat(_) => Protocol::OpenAiChat,
            Self::Responses(_) => Protocol::OpenAiResponses,
            Self::Messages(_) => Protocol::AnthropicMessages,
            Self::Gemini(_) => Protocol::Gemini,
        }
    }

    /// 取出完整协议响应，供 HTTP 边界序列化。
    pub(crate) fn into_body(self) -> serde_json::Result<serde_json::Value> {
        match self {
            Self::Chat(body) => serde_json::to_value(body),
            Self::Responses(body) => serde_json::to_value(body),
            Self::Messages(body) => serde_json::to_value(body),
            Self::Gemini(body) => serde_json::to_value(body),
        }
    }
}
