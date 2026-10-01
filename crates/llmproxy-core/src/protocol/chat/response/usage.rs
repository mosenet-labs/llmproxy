//! Chat Completions 非流式响应和流式用量分片共用的词元统计。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::optional_nullable::OptionalNullable;

/// 一次完成的输入、输出及总词元数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 生成回复使用的词元数。
    pub completion_tokens: u64,
    /// 输入消息使用的词元数。
    pub prompt_tokens: u64,
    /// 输入和输出合计使用的词元数。
    pub total_tokens: u64,
    /// 输出词元的细分统计。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub completion_tokens_details: OptionalNullable<CompletionTokensDetails>,
    /// 输入词元的细分统计，包括缓存读取和写入。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_tokens_details: OptionalNullable<PromptTokensDetails>,
    /// 保留未声明的用量字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出词元的类型分布。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionTokensDetails {
    /// 与预测输出匹配的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub accepted_prediction_tokens: OptionalNullable<u64>,
    /// 输出音频词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio_tokens: OptionalNullable<u64>,
    /// 推理词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub reasoning_tokens: OptionalNullable<u64>,
    /// 未匹配预测输出、但仍计入生成用量的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub rejected_prediction_tokens: OptionalNullable<u64>,
    /// 输出文本词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub text_tokens: OptionalNullable<u64>,
    /// 保留未声明的输出统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入词元的类型分布和缓存统计。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptTokensDetails {
    /// 输入音频词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio_tokens: OptionalNullable<u64>,
    /// 写入提示缓存的输入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_write_tokens: OptionalNullable<u64>,
    /// 从提示缓存读取的输入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cached_tokens: OptionalNullable<u64>,
    /// 输入图片词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub image_tokens: OptionalNullable<u64>,
    /// 输入文本词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub text_tokens: OptionalNullable<u64>,
    /// 保留未声明的输入统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
