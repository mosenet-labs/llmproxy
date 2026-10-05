//! 四协议的单个流式载体；SSE 分帧和 JSON 字节读写由 HTTP 边界负责。
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::{Protocol, chat, gemini, messages, responses};

/// 单个已解码的协议事件，作为流式 codec 的输入或输出。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Chat 的候选增量和累计用量。
    Chat(Box<chat::response::Chunk>),
    /// Responses 的语义事件。
    Responses(Box<responses::response::Event>),
    /// Messages 的消息或内容块事件。
    Messages(Box<messages::response::Event>),
    /// Gemini 使用生成响应结构表示每个分片。
    Gemini(Box<gemini::response::Response>),
    /// HTTP 边界识别的结束标记或正常 EOF，不作为 JSON 序列化发送。
    End(Protocol),
}

impl Event {
    /// 返回事件所使用的协议。
    pub fn protocol(&self) -> Protocol {
        match self {
            Self::Chat(_) => Protocol::OpenAiChat,
            Self::Responses(_) => Protocol::OpenAiResponses,
            Self::Messages(_) => Protocol::AnthropicMessages,
            Self::Gemini(_) => Protocol::Gemini,
            Self::End(protocol) => *protocol,
        }
    }
}

/// 未声明的事件只保留事件名和扩展；跨协议处理需要单独决定其语义。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnknownEvent {
    /// 未知事件的类型，不能由其他字段猜测。
    pub r#type: String,
    /// 未建模事件的原始字段。
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// 先按类型化事件解码，仅真正未知的事件允许落入开放扩展。
/// 不通过整个事件的 Value 做已知协议的中转。
pub(crate) fn decode_open<'de, D, T>(
    deserializer: D,
    is_known: fn(&str) -> bool,
) -> Result<Open<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire<T> {
        Known(Box<T>),
        Other(UnknownEvent),
    }
    match Wire::<T>::deserialize(deserializer)? {
        Wire::Known(event) => Ok(Open::Known(event)),
        Wire::Other(event) if is_known(&event.r#type) => Err(serde::de::Error::custom(
            "已知流式事件缺少必需字段或字段类型错误",
        )),
        Wire::Other(event) => Ok(Open::Other(event)),
    }
}

/// 两套开放事件解码器共用的内部结果。
pub(crate) enum Open<T> {
    Known(Box<T>),
    Other(UnknownEvent),
}

/// 事件名、Serde 标签和未知事件检查从同一声明生成，避免三份列表漂移。
macro_rules! typed_events {
    ($(#[$meta:meta])* pub enum $name:ident {
        $($(#[$variant_meta:meta])* $variant:ident($payload:ty) = $kind:literal,)*
    }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type")]
        pub enum $name {
            $($(#[$variant_meta])* #[serde(rename = $kind)] $variant($payload),)*
        }
        impl $name {
            /// 判断事件名是否已有类型化声明。
            pub(crate) fn is_known(kind: &str) -> bool {
                matches!(kind, $($kind)|*)
            }
            /// 返回线缆报文中的事件类型。
            pub fn kind(&self) -> &'static str {
                match self {$(Self::$variant(_) => $kind,)*}
            }
            /// 各事件共用扩展字段诊断入口，不序列化整个载体。
            pub(crate) fn extra(&self) -> &serde_json::Map<String, serde_json::Value> {
                match self {$(Self::$variant(payload) => &payload.extra,)*}
            }
        }
    };
}
pub(crate) use typed_events;
