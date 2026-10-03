//! Responses 非流式完整响应；流式 `response.*` 事件可携带同一结构。
//! 参考 API：https://developers.openai.com/api/reference/cli/resources/responses/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{message::Message, usage::Usage};
use crate::protocol::{
    moderation::Moderation,
    optional_nullable::OptionalNullable,
    responses::request::body::{ConversationId, Prompt, PromptCacheOptions, Reasoning, TextConfig},
};

/// 完整响应外壳；输出项的开放联合由 `OutputItem` 保留。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// 响应 ID。
    pub id: String,
    /// 创建时间的 Unix 秒级时间戳。
    pub created_at: i64,
    /// 错误详情。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub error: OptionalNullable<ResponseError>,
    /// 未完成原因。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub incomplete_details: OptionalNullable<IncompleteDetails>,
    /// 系统或开发者指令；返回形状可包含输入项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub instructions: OptionalNullable<Value>,
    /// 附加元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub metadata: OptionalNullable<Map<String, Value>>,
    /// 实际执行的模型 ID。
    pub model: String,
    /// 对象类型，标准值为 `response`。
    pub object: String,
    /// 模型生成的有序输出项。
    pub output: Vec<OutputItem>,
    /// 是否允许并行工具调用。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parallel_tool_calls: OptionalNullable<bool>,
    /// 实际使用的采样温度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub temperature: OptionalNullable<f64>,
    /// 实际使用的工具选择。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_choice: OptionalNullable<Value>,
    /// 本次可用工具定义。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tools: OptionalNullable<Vec<crate::protocol::responses::function::Tool>>,
    /// 实际使用的核采样阈值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_p: OptionalNullable<f64>,
    /// 是否在后台生成。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub background: OptionalNullable<bool>,
    /// 完成时间。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub completed_at: OptionalNullable<i64>,
    /// 响应所属会话。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub conversation: OptionalNullable<ConversationId>,
    /// 最大输出词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_output_tokens: OptionalNullable<u64>,
    /// 最大工具调用数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_tool_calls: OptionalNullable<u64>,
    /// 输入和输出的审核结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub moderation: OptionalNullable<Moderation>,
    /// 上一次响应的 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub previous_response_id: OptionalNullable<String>,
    /// 使用的提示模板。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt: OptionalNullable<Prompt>,
    /// 缓存命中诊断；不同结果类型保留原始字段。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_diagnostics: OptionalNullable<Map<String, Value>>,
    /// 提示缓存的分组键。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_key: OptionalNullable<String>,
    /// 实际使用的缓存设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_options: OptionalNullable<PromptCacheOptions>,
    /// 旧版缓存保留策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_retention: OptionalNullable<String>,
    /// 实际使用的推理设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub reasoning: OptionalNullable<Reasoning>,
    /// 实际使用的服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 响应状态，例如 `completed` 或 `incomplete`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub status: OptionalNullable<String>,
    /// 文本输出设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub text: OptionalNullable<TextConfig>,
    /// 截断策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub truncation: OptionalNullable<String>,
    /// 输入、输出及缓存词元用量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub usage: OptionalNullable<Usage>,
    /// 保留新增响应字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出消息已强类型化，其他工具和推理输出项保持原始对象。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum OutputItem {
    /// 独立客户端函数调用。
    FunctionCall(crate::protocol::responses::function::Call),
    /// 助手输出消息。
    Message(Message),
    /// 工具调用、推理或其他输出项。
    Other(Map<String, Value>),
}
/// 按显式类型选择工具项，已知类型无效时不能降为未知扩展。
impl<'de> serde::Deserialize<'de> for OutputItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let kind = value.get("type").and_then(Value::as_str);
        match kind {
            Some("function_call") => serde_json::from_value(value)
                .map(Self::FunctionCall)
                .map_err(serde::de::Error::custom),
            _ => {
                if let Ok(message) = serde_json::from_value(value.clone()) {
                    return Ok(Self::Message(message));
                }
                match value {
                    Value::Object(value) => Ok(Self::Other(value)),
                    _ => Err(serde::de::Error::custom("协议项必须是对象")),
                }
            }
        }
    }
}

/// 生成失败时返回的错误。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
    /// 错误代码。
    pub code: String,
    /// 错误说明。
    pub message: String,
    /// 可选的对齐错误详情。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub misalignment: OptionalNullable<Value>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 响应未完成的原因。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IncompleteDetails {
    /// 未完成原因，例如达到输出长度限制。
    pub reason: String,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::Response;
    use serde_json::json;

    #[test]
    fn response_round_trip_with_tool_output_and_usage() {
        let source = json!({
            "id":"resp_1","created_at":123,"model":"m","object":"response",
            "output":[
                {"id":"msg_1","content":[{"type":"output_text","annotations":[],"text":"hello"}],"role":"assistant","status":"completed","type":"message"},
                {"type":"function_call","id":"fc_1","call_id":"c1","name":"lookup","arguments":"{}","status":"completed"}
            ],
            "status":"completed","usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":4,"cache_write_tokens":2},"output_tokens":3,"output_tokens_details":{"reasoning_tokens":1},"total_tokens":13},
            "prompt_cache_options":{"mode":"explicit","ttl":"30m","comparison_response_id":"resp_0"}
        });
        let response: Response = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), source);
    }
}
