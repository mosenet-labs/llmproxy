//! 各协议的原始数据结构和协议标识。

pub mod chat;

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
