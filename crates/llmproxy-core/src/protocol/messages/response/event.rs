//! Messages SSE 的单个 `data` 事件；用量增量不可当作完整用量。
//! 参考 API：https://platform.claude.com/docs/en/build-with-claude/streaming

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{message::Message, usage::Usage};
use crate::protocol::OptionalNullable;

/// 一条流式事件；开放事件类型及字段继续保留原值。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// `message_start`、`content_block_delta` 等事件类型。
    pub r#type: String,
    /// `message_start` 中的消息快照。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub message: OptionalNullable<Message>,
    /// 内容块在消息中的索引。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub index: OptionalNullable<u64>,
    /// 新增的完整内容块。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub content_block: OptionalNullable<Value>,
    /// 内容块或消息的局部变化。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub delta: OptionalNullable<Delta>,
    /// `message_delta` 中的累计用量；可能只有输出计数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub usage: OptionalNullable<DeltaUsage>,
    /// 流内错误详情。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub error: OptionalNullable<Value>,
    /// 保留未知事件的专有字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 内容块或消息的增量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// `text_delta`、`input_json_delta`、`thinking_delta` 等类型。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub r#type: OptionalNullable<String>,
    /// 新增文本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub text: OptionalNullable<String>,
    /// 工具参数 JSON 的局部字符串。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub partial_json: OptionalNullable<String>,
    /// 新增思考文本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thinking: OptionalNullable<String>,
    /// 思考签名的增量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub signature: OptionalNullable<String>,
    /// 消息级停止原因。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_reason: OptionalNullable<String>,
    /// 命中的停止序列。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_sequence: OptionalNullable<String>,
    /// 保留其他增量字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 流式事件中的局部累计用量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaUsage {
    /// 更新后的未缓存输入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub input_tokens: OptionalNullable<u64>,
    /// 更新后的输出词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub output_tokens: OptionalNullable<u64>,
    /// 新增或更新的缓存写入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_creation_input_tokens: OptionalNullable<u64>,
    /// 新增或更新的缓存读取词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_read_input_tokens: OptionalNullable<u64>,
    /// 保留其他累计统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 提取 `message_start` 的完整用量；其他事件需按累计语义合并。
pub fn start_usage(event: &Event) -> Option<&Usage> {
    match &event.message {
        OptionalNullable::Value(message) if event.r#type == "message_start" => Some(&message.usage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Event;
    use serde_json::json;

    #[test]
    fn streaming_events_keep_partial_usage_distinct() {
        let events = [
            json!({"type":"message_start","message":{"id":"m1","type":"message","role":"assistant","content":[],"model":"claude","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":4}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3}}),
        ];
        for source in events {
            let event: Event = serde_json::from_value(source.clone()).unwrap();
            assert_eq!(serde_json::to_value(event).unwrap(), source);
        }
    }
}
