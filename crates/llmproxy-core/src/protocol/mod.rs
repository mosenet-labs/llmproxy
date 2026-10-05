//! 各协议的原始数据结构和协议标识。

pub mod chat;
mod common;
pub mod gemini;
pub mod messages;
pub mod moderation;
mod optional_nullable;
mod request;
mod response;
pub mod responses;
pub mod stream;
pub mod wire;
pub use request::Request;
pub use response::Response;

pub use optional_nullable::OptionalNullable;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessagesAuth {
    #[default]
    ApiKey,
    Bearer,
}

impl MessagesAuth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "x-api-key",
            Self::Bearer => "bearer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "x-api-key" => Some(Self::ApiKey),
            "bearer" => Some(Self::Bearer),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    OpenAiChat,
    OpenAiResponses,
    AnthropicMessages,
    Gemini,
}

impl Protocol {
    /// 将路由和页面使用的稳定协议标识解析为类型，不依赖枚举的序列化名称。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_chat" => Some(Self::OpenAiChat),
            "openai_responses" => Some(Self::OpenAiResponses),
            "anthropic_messages" => Some(Self::AnthropicMessages),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }

    pub const fn upstream_path(self) -> &'static str {
        match self {
            Self::OpenAiChat => "/v1/chat/completions",
            Self::OpenAiResponses => "/v1/responses",
            Self::AnthropicMessages => "/v1/messages",
            Self::Gemini => "/v1beta/models",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai_chat",
            Self::OpenAiResponses => "openai_responses",
            Self::AnthropicMessages => "anthropic_messages",
            Self::Gemini => "gemini",
        }
    }
}
