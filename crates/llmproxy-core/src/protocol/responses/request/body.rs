//! `POST /responses` 请求正文；流式与非流式共用。
//! 参考 API：https://developers.openai.com/api/reference/resources/responses/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::message::Message;
use crate::protocol::{moderation::ModerationSettings, optional_nullable::OptionalNullable};

/// Responses 请求的完整外壳；开放的工具和输入项保留其原始字段。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 领域访问配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub access_programs: OptionalNullable<Map<String, Value>>,
    /// 是否在后台运行。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub background: OptionalNullable<bool>,
    /// 上下文压缩策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub context_management: OptionalNullable<Vec<ContextManagement>>,
    /// 关联的会话 ID 或会话对象。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub conversation: OptionalNullable<Conversation>,
    /// 要在输出中附带的额外数据类型。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub include: OptionalNullable<Vec<String>>,
    /// 输入文本或有序输入项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub input: OptionalNullable<Input>,
    /// 加在上下文前的指令。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub instructions: OptionalNullable<String>,
    /// 最大输出词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_output_tokens: OptionalNullable<u64>,
    /// 本次响应允许的最大工具调用数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_tool_calls: OptionalNullable<u64>,
    /// 响应附加的字符串元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub metadata: OptionalNullable<Map<String, Value>>,
    /// 生成模型 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub model: OptionalNullable<String>,
    /// 输入和输出的审核配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub moderation: OptionalNullable<ModerationSettings>,
    /// 是否允许并行调用工具。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parallel_tool_calls: OptionalNullable<bool>,
    /// 上一次响应的 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub previous_response_id: OptionalNullable<String>,
    /// 可复用的提示模板。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt: OptionalNullable<Prompt>,
    /// 提示缓存的分组键。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_key: OptionalNullable<String>,
    /// 提示缓存模式、TTL 和诊断选项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_options: OptionalNullable<PromptCacheOptions>,
    /// 旧版缓存保留策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_retention: OptionalNullable<String>,
    /// 推理配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub reasoning: OptionalNullable<Reasoning>,
    /// 终端用户的安全标识。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub safety_identifier: OptionalNullable<String>,
    /// 请求的服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 是否保存响应供后续查询。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub store: OptionalNullable<bool>,
    /// 是否使用 SSE 输出。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stream: OptionalNullable<bool>,
    /// 流式事件的附加选项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stream_options: OptionalNullable<StreamOptions>,
    /// 采样温度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub temperature: OptionalNullable<f64>,
    /// 文本输出格式和详细程度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub text: OptionalNullable<TextConfig>,
    /// 目标工具或工具选择模式；具体工具联合保持开放。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_choice: OptionalNullable<Value>,
    /// 函数、内置工具或 MCP 工具的声明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tools: OptionalNullable<Vec<crate::protocol::responses::function::Tool>>,
    /// 每个输出词元最多返回的候选对数概率数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_logprobs: OptionalNullable<u64>,
    /// 核采样阈值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_p: OptionalNullable<f64>,
    /// 输入超出上下文窗口时的截断策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub truncation: OptionalNullable<String>,
    /// 旧版终端用户 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub user: OptionalNullable<String>,
    /// 保留新增请求参数。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入可以是纯文本或按顺序排列的消息、工具等输入项。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Input {
    /// 纯文本输入。
    Text(String),
    /// 混合输入项。
    Items(Vec<InputItem>),
}

/// 输入项中的消息已强类型化；其他工具输入项保留原对象。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum InputItem {
    /// 独立客户端函数调用。
    FunctionCall(crate::protocol::responses::function::Call),
    /// 客户端返回的函数结果。
    FunctionCallOutput(crate::protocol::responses::function::CallOutput),
    /// 消息输入项。
    Message(Message),
    /// 函数调用、函数结果等其他输入项。
    Other(Map<String, Value>),
}
/// 按显式类型选择工具项，已知类型无效时不能降为未知扩展。
impl<'de> serde::Deserialize<'de> for InputItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let kind = value.get("type").and_then(Value::as_str);
        match kind {
            Some("function_call") => serde_json::from_value(value)
                .map(Self::FunctionCall)
                .map_err(serde::de::Error::custom),
            Some("function_call_output") => serde_json::from_value(value)
                .map(Self::FunctionCallOutput)
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

/// 上下文管理的一条规则。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextManagement {
    /// 规则类型，例如 `compaction`。
    pub r#type: String,
    /// 自动压缩阈值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub compact_threshold: OptionalNullable<u64>,
    /// 保留新增规则参数。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 会话 ID 或包含 ID 的对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Conversation {
    /// 会话 ID。
    Id(String),
    /// 会话引用对象。
    Object(ConversationId),
}

/// 会话引用对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConversationId {
    /// 会话 ID。
    pub id: String,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 可复用提示模板。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    /// 模板 ID。
    pub id: String,
    /// 模板变量，可包含非文本输入。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub variables: OptionalNullable<Map<String, Value>>,
    /// 模板版本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub version: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 提示缓存配置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptCacheOptions {
    /// 用于缓存诊断对比的响应 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub comparison_response_id: OptionalNullable<String>,
    /// 隐式或显式缓存断点模式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub mode: OptionalNullable<String>,
    /// 是否提前准备缓存。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prewarm: OptionalNullable<bool>,
    /// 缓存条目的最低存活时间。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub ttl: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 推理模型的预算及摘要设置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reasoning {
    /// 可复用的推理上下文。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub context: OptionalNullable<Value>,
    /// 推理强度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub effort: OptionalNullable<String>,
    /// 是否生成推理摘要。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub generate_summary: OptionalNullable<String>,
    /// 摘要详细程度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub summary: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 流式响应选项。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamOptions {
    /// 是否在事件中包含混淆数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub include_obfuscation: OptionalNullable<bool>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 文本输出配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextConfig {
    /// 普通文本或 JSON Schema 输出格式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub format: OptionalNullable<TextFormat>,
    /// 输出详细程度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub verbosity: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 文本格式的已知参数；JSON Schema 本身为开放对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextFormat {
    /// 格式类型，例如 `text` 或 `json_schema`。
    pub r#type: String,
    /// 结构化格式名称。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub name: OptionalNullable<String>,
    /// 结构化格式说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 调用方提供的 JSON Schema。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub schema: OptionalNullable<Map<String, Value>>,
    /// 是否严格使用 Schema。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub strict: OptionalNullable<bool>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::Request;
    use serde_json::json;

    #[test]
    fn request_round_trip_with_mixed_input_and_cache() {
        let source = json!({
            "model":"m","input":[
                {"role":"user","content":[{"type":"input_text","text":"hi","prompt_cache_breakpoint":{"mode":"explicit"}}]},
                {"type":"function_call_output","call_id":"c1","output":"done"}
            ],
            "prompt_cache_key":"session",
            "prompt_cache_options":{"mode":"explicit","ttl":"30m","prewarm":true},
            "reasoning":{"effort":"medium","summary":"auto"},
            "text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object"},"strict":true}},
            "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],
            "stream":true,"vendor":1
        });
        let request: Request = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }
}
