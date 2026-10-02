//! Responses 输入、输出及缓存词元统计。
//! 参考 API：https://developers.openai.com/api/reference/cli/resources/responses/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::optional_nullable::OptionalNullable;

/// 一次响应的词元用量。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入词元数，包含命中的缓存内容。
    pub input_tokens: u64,
    /// 输入缓存的读取与写入细项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub input_tokens_details: OptionalNullable<InputTokensDetails>,
    /// 输出词元数。
    pub output_tokens: u64,
    /// 输出推理词元细项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub output_tokens_details: OptionalNullable<OutputTokensDetails>,
    /// 输入与输出的总词元数。
    pub total_tokens: u64,
    /// 保留供应商新增统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入缓存用量。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputTokensDetails {
    /// 本次写入提示缓存的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_write_tokens: OptionalNullable<u64>,
    /// 从提示缓存读取的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cached_tokens: OptionalNullable<u64>,
    /// 保留新增输入统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出词元细项。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    /// 模型内部推理使用的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub reasoning_tokens: OptionalNullable<u64>,
    /// 保留新增输出统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
