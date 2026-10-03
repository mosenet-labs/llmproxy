//! Gemini `GenerateContentResponse`；流式接口逐块使用同一 JSON 结构。
//! 参考 API：https://ai.google.dev/api/generate-content#GenerateContentResponse

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{message::Message, usage::UsageMetadata};
use crate::protocol::OptionalNullable;

/// Gemini 的一次生成响应或一个流式响应块。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// 候选输出；输入被阻断时可以缺失。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub candidates: OptionalNullable<Vec<Candidate>>,
    /// 输入内容的安全过滤结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_feedback: OptionalNullable<PromptFeedback>,
    /// 输入、输出及缓存词元统计。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub usage_metadata: OptionalNullable<UsageMetadata>,
    /// 实际模型版本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub model_version: OptionalNullable<String>,
    /// 响应 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_id: OptionalNullable<String>,
    /// 模型的当前生命周期状态。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub model_status: OptionalNullable<ModelStatus>,
    /// 保留新增响应字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个模型候选输出。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// 已生成的内容。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub content: OptionalNullable<Message>,
    /// 候选输出停止原因。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub finish_reason: OptionalNullable<String>,
    /// 各安全类别的评级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub safety_ratings: OptionalNullable<Vec<SafetyRating>>,
    /// 引用元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub citation_metadata: OptionalNullable<Map<String, Value>>,
    /// 此候选的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub token_count: OptionalNullable<u64>,
    /// 旧版引用归属信息。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub grounding_attributions: OptionalNullable<Vec<Value>>,
    /// 搜索等工具提供的依据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub grounding_metadata: OptionalNullable<Map<String, Value>>,
    /// 平均对数概率。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub avg_logprobs: OptionalNullable<f64>,
    /// 各输出词元的对数概率详情。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub logprobs_result: OptionalNullable<Map<String, Value>>,
    /// URL 上下文工具的调用元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub url_context_metadata: OptionalNullable<Map<String, Value>>,
    /// 候选输出索引。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub index: OptionalNullable<u64>,
    /// 停止原因的补充说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub finish_message: OptionalNullable<String>,
    /// 保留新增候选字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入被过滤时的反馈。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptFeedback {
    /// 阻断原因。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub block_reason: OptionalNullable<String>,
    /// 各类别的安全评级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub safety_ratings: OptionalNullable<Vec<SafetyRating>>,
    /// 保留新增反馈字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 内容在一个安全类别上的评级。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyRating {
    /// 安全类别。
    pub category: String,
    /// 风险概率等级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub probability: OptionalNullable<String>,
    /// 原始概率分数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub probability_score: OptionalNullable<f64>,
    /// 风险严重程度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub severity: OptionalNullable<String>,
    /// 原始严重程度分数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub severity_score: OptionalNullable<f64>,
    /// 是否被此类别阻断。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub blocked: OptionalNullable<bool>,
    /// 保留新增评级字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 底层模型的生命周期信息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    /// 模型所处阶段。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub model_stage: OptionalNullable<String>,
    /// 模型停用时间。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub retirement_time: OptionalNullable<String>,
    /// 状态说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub message: OptionalNullable<String>,
    /// 保留新增状态字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::Response;
    use serde_json::json;

    #[test]
    fn response_round_trip_with_candidate_and_usage() {
        let source = json!({
            "candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP","safetyRatings":[{"category":"HARM_CATEGORY_HATE_SPEECH","probability":"NEGLIGIBLE","blocked":false}],"index":0}],
            "usageMetadata":{"promptTokenCount":10,"cachedContentTokenCount":4,"candidatesTokenCount":3,"thoughtsTokenCount":2,"totalTokenCount":15,"promptTokensDetails":[{"modality":"TEXT","tokenCount":8},{"modality":"IMAGE","tokenCount":2}],"cacheTokensDetails":[{"modality":"TEXT","tokenCount":4}]},
            "modelVersion":"gemini","responseId":"r1"
        });
        let response: Response = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), source);
    }
}
