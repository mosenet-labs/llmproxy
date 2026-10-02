//! Responses SSE 事件的 JSON `data` 载体；每个事件独立解码。
//! 参考 API：https://developers.openai.com/api/reference/responses-streaming

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::body::{OutputItem, Response};
use crate::protocol::optional_nullable::OptionalNullable;

/// 保留事件类型和公共索引；事件特有字段由 `extra` 原样承载。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// 事件类型，例如 `response.output_text.delta` 或 `response.completed`。
    pub r#type: String,
    /// 事件顺序号。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub sequence_number: OptionalNullable<u64>,
    /// 生命周期事件中的响应快照。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response: OptionalNullable<Response>,
    /// 输出项事件中的项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub item: OptionalNullable<OutputItem>,
    /// 输出项的索引。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub output_index: OptionalNullable<u64>,
    /// 内容块的索引。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub content_index: OptionalNullable<u64>,
    /// 输出项 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub item_id: OptionalNullable<String>,
    /// 文本或参数的增量；按事件类型解释。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub delta: OptionalNullable<Value>,
    /// 保留不同事件类型的特有字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
