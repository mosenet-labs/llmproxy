//! Gemini 生成响应中的缓存、模态及词元用量。
//! 参考 API：https://ai.google.dev/api/generate-content#UsageMetadata

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::OptionalNullable;

/// 一次生成请求的词元统计；流式每个响应块也可带有此字段。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    /// 输入总词元数，已包含缓存命中的内容。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_token_count: OptionalNullable<u64>,
    /// 其中从缓存读取的输入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cached_content_token_count: OptionalNullable<u64>,
    /// 所有候选回复的输出词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub candidates_token_count: OptionalNullable<u64>,
    /// 工具调用提示使用的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_use_prompt_token_count: OptionalNullable<u64>,
    /// 模型思考使用的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thoughts_token_count: OptionalNullable<u64>,
    /// 本次请求的总词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub total_token_count: OptionalNullable<u64>,
    /// 输入各模态的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_tokens_details: OptionalNullable<Vec<ModalityTokenCount>>,
    /// 缓存输入各模态的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_tokens_details: OptionalNullable<Vec<ModalityTokenCount>>,
    /// 输出各模态的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub candidates_tokens_details: OptionalNullable<Vec<ModalityTokenCount>>,
    /// 工具调用提示各模态的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_use_prompt_tokens_details: OptionalNullable<Vec<ModalityTokenCount>>,
    /// 实际服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 保留新增统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 单一输入或输出模态的词元数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModalityTokenCount {
    /// `TEXT`、`IMAGE`、`AUDIO` 等模态。
    pub modality: String,
    /// 对应的词元数。
    pub token_count: u64,
    /// 保留新增模态字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
