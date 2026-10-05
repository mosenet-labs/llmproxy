//! Messages SSE 的类型化事件；局部累计用量与完整响应的用量分别声明。
//! 参考 API：https://platform.claude.com/docs/en/build-with-claude/streaming
use super::{
    message::{ContentBlock, Message},
    usage::{CacheCreation, OutputTokensDetails, ServerToolUse, Usage},
};
use crate::protocol::{
    OptionalNullable as O,
    stream::{self, UnknownEvent},
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// 已知事件严格解码；未知事件仅用于同协议保留或跨协议诊断。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Event {
    /// 已声明的消息、内容块、心跳和错误事件。
    Known(Box<KnownEvent>),
    /// Provider 新增的事件。
    Other(UnknownEvent),
}
impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match stream::decode_open(deserializer, KnownEvent::is_known)? {
            stream::Open::Known(event) => Ok(Self::Known(event)),
            stream::Open::Other(event) => Ok(Self::Other(event)),
        }
    }
}
stream::typed_events! {
    /// Messages 的事件标签及各自必需载体。
    pub enum KnownEvent {
        /// 消息开始，包含初始响应和输入用量。
        MessageStart(MessageStart) = "message_start",
        /// 新内容块开始。
        ContentBlockStart(ContentBlockStart) = "content_block_start",
        /// 一个内容块的局部增量。
        ContentBlockDelta(ContentBlockDelta) = "content_block_delta",
        /// 当前内容块生成结束。
        ContentBlockStop(ContentBlockStop) = "content_block_stop",
        /// 停止原因和累计用量更新。
        MessageDelta(MessageDeltaEvent) = "message_delta",
        /// 整条消息结束。
        MessageStop(Empty) = "message_stop",
        /// 连接心跳，不产生模型正文。
        Ping(Empty) = "ping",
        /// 流内失败，可能发生在 HTTP 响应头发送后。
        Error(ErrorEvent) = "error",
    }
}
/// 消息开始的响应快照。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageStart {
    /// 空内容或初始内容的完整消息。
    pub message: Message,
    /// 未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 新建内容块，工具参数可能仍为空对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockStart {
    /// 内容块在消息中的索引。
    pub index: u64,
    /// 原生响应内容块，不把已知块声明为整块 Value。
    pub content_block: ContentBlock,
    /// 未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 内容块增量只影响指定索引，不能按完整消息覆盖。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockDelta {
    /// 被更新的内容块索引。
    pub index: u64,
    /// 本次文本、参数、思考或签名的增量。
    pub delta: Delta,
    /// 未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 内容块结束，不隐含整个响应结束。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockStop {
    /// 结束的内容块索引。
    pub index: u64,
    /// 未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 消息状态和累计统计更新。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaEvent {
    /// 消息级停止原因及停止序列。
    pub delta: MessageDelta,
    /// Provider 本次报告的局部累计统计。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub usage: O<DeltaUsage>,
    /// 未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 消息级更新没有内容块增量的 type 字段。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageDelta {
    /// 停止原因。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub stop_reason: O<String>,
    /// 命中的停止序列。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub stop_sequence: O<String>,
    /// 容器、停止详情等事件扩展。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 已知增量严格解码，未知增量按官方向前兼容规则保留。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Delta {
    /// 已声明的增量语义。
    Known(Box<KnownDelta>),
    /// Provider 新增的增量，不丢失其字段。
    Other(UnknownEvent),
}

impl<'de> Deserialize<'de> for Delta {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match stream::decode_open(deserializer, KnownDelta::is_known)? {
            stream::Open::Known(delta) => Ok(Self::Known(delta)),
            stream::Open::Other(delta) => Ok(Self::Other(delta)),
        }
    }
}

/// 文本、函数参数、引用和思考各有独立载体，参数分片不要求是完整 JSON。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownDelta {
    /// 追加可见文本。
    TextDelta {
        /// 本次新增文本。
        text: String,
        /// 未声明的增量字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 追加工具参数字符串。
    InputJsonDelta {
        /// 可能不完整的 JSON 字符串。
        partial_json: String,
        /// 未声明的增量字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 追加模型思考文本。
    ThinkingDelta {
        /// 本次新增思考文本。
        thinking: String,
        /// 未声明的增量字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 追加不透明签名，不能当作可见思考文本。
    SignatureDelta {
        /// 本次新增签名数据。
        signature: String,
        /// 未声明的增量字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 向当前文本块追加一条引用。
    /// 参考：https://platform.claude.com/docs/en/build-with-claude/citations
    CitationsDelta {
        /// 不同文档定位方式的开放引用叶子，与非流式 citations 保持一致。
        citation: Value,
        /// 未声明的增量字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

impl KnownDelta {
    /// 已知标签解码失败时不能降级为未知增量。
    fn is_known(kind: &str) -> bool {
        matches!(
            kind,
            "text_delta"
                | "input_json_delta"
                | "thinking_delta"
                | "signature_delta"
                | "citations_delta"
        )
    }
}
/// 没有已声明载体的心跳或消息结束。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Empty {
    /// 仍保留事件扩展。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 流内错误的载体。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorEvent {
    /// Provider 的错误类型与说明。
    pub error: StreamError,
    /// 未声明的事件字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// Messages 错误详情，不与普通内容块混用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamError {
    /// 错误类别，例如 overloaded_error。
    pub r#type: String,
    /// Provider 返回的错误说明；日志不得直接打印该字段。
    pub message: String,
    /// 未声明的错误字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 流式事件中的局部累计用量，未报告的字段保持缺失。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeltaUsage {
    /// 按 TTL 划分的缓存写入量。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub cache_creation: O<CacheCreation>,
    /// 写入缓存的累计输入词元数。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub cache_creation_input_tokens: O<u64>,
    /// 缓存读取的累计输入词元数。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub cache_read_input_tokens: O<u64>,
    /// 未缓存的累计输入词元数。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub input_tokens: O<u64>,
    /// 累计输出词元数。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub output_tokens: O<u64>,
    /// 推理等输出细项。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub output_tokens_details: O<OutputTokensDetails>,
    /// 服务端工具请求次数。
    #[serde(default, skip_serializing_if = "O::is_missing")]
    pub server_tool_use: O<ServerToolUse>,
    /// 未声明的累计统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 提取 message_start 的完整初始用量；后续事件按累计语义合并。
pub fn start_usage(event: &Event) -> Option<&Usage> {
    match event {
        Event::Known(event) => match event.as_ref() {
            KnownEvent::MessageStart(start) => Some(&start.message.usage),
            _ => None,
        },
        Event::Other(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, KnownEvent};
    use serde_json::json;
    #[test]
    fn typed_events_keep_partial_parameters_signatures_and_cache_usage() {
        for source in [
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call","name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":{"type":"char_location","cited_text":"source","document_index":0,"start_char_index":0,"end_char_index":6}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"future_delta","future":{"enabled":true}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3,"cache_creation":{"ephemeral_1h_input_tokens":4},"output_tokens_details":{"thinking_tokens":2}}}),
            json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}}),
            json!({"type":"vendor_event","payload":{"future":true}}),
        ] {
            let event: Event =
                serde_json::from_slice(&serde_json::to_vec(&source).unwrap()).unwrap();
            assert_eq!(serde_json::to_value(&event).unwrap(), source);
            if source["type"] == "message_delta" {
                assert!(
                    matches!(event, Event::Known(event) if matches!(*event, KnownEvent::MessageDelta(_)))
                );
            }
        }
    }
    #[test]
    fn known_malformed_events_cannot_become_unknown_extensions() {
        for source in [
            json!({"type":"content_block_start","index":0}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":4}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"citations_delta"}}),
            json!({"type":"message_start","message":{}}),
            json!({"type":"error","error":{"message":"busy"}}),
        ] {
            assert!(serde_json::from_value::<Event>(source).is_err());
        }
    }
}
