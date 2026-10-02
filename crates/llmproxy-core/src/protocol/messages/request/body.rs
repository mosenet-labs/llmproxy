//! `POST /v1/messages` 的请求正文。
//! 参考 API：https://platform.claude.com/docs/en/api/messages/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{cache::CacheControl, message::Message};
use crate::protocol::OptionalNullable;

/// Messages 请求外壳，流式与非流式请求使用同一结构。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 模型最多生成的词元数。
    pub max_tokens: u64,
    /// 按顺序排列的对话历史和最新输入。
    pub messages: Vec<Message>,
    /// 生成模型 ID。
    pub model: String,
    /// 自动标记最后一个可缓存内容块的缓存配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_control: OptionalNullable<CacheControl>,
    /// 可复用的容器 ID 及技能设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub container: OptionalNullable<Container>,
    /// 缓存诊断配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub diagnostics: OptionalNullable<Diagnostics>,
    /// 推理位置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub inference_geo: OptionalNullable<String>,
    /// 与最终用户关联的元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub metadata: OptionalNullable<Metadata>,
    /// 模型输出格式与推理强度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub output_config: OptionalNullable<OutputConfig>,
    /// 请求的服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 自定义停止序列。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_sequences: OptionalNullable<Vec<String>>,
    /// 是否以 SSE 增量输出。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stream: OptionalNullable<bool>,
    /// 系统提示文本或带缓存断点的文本块。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub system: OptionalNullable<SystemPrompt>,
    /// 采样温度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub temperature: OptionalNullable<f64>,
    /// 思考模式与预算。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thinking: OptionalNullable<Thinking>,
    /// 工具选择模式或指定工具。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_choice: OptionalNullable<ToolChoice>,
    /// 客户端工具或服务端工具声明；具体工具类型保持开放。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tools: OptionalNullable<Vec<Map<String, Value>>>,
    /// 从候选词元中采样的数量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_k: OptionalNullable<u64>,
    /// 核采样阈值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_p: OptionalNullable<f64>,
    /// 保留新增请求字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 纯文本或多段系统提示。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// 单段系统提示。
    Text(String),
    /// 多段系统提示，可分别指定缓存断点。
    Parts(Vec<SystemText>),
}

/// 一段系统提示文本。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemText {
    /// 文本内容。
    pub text: String,
    /// 内容块类型，标准值为 `text`。
    pub r#type: String,
    /// 内容末尾的缓存断点。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_control: OptionalNullable<CacheControl>,
    /// 引用标注。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub citations: OptionalNullable<Vec<Value>>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 容器复用及技能设置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Container {
    /// 已有容器的 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 要加载的技能。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub skills: OptionalNullable<Vec<Skill>>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 容器内的一项技能。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    /// 技能来源类型。
    pub r#type: String,
    /// 技能 ID。
    pub skill_id: String,
    /// 技能版本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub version: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 请求的缓存命中诊断输入。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    /// 用于比较缓存前缀的上一条消息 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub previous_message_id: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 与最终用户关联的标识。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// 应用中的不透明用户 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub user_id: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出格式与模型投入程度。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputConfig {
    /// 输出推理强度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub effort: OptionalNullable<String>,
    /// JSON Schema 等结构化输出格式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub format: OptionalNullable<OutputFormat>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 结构化输出格式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputFormat {
    /// 格式类型，标准值为 `json_schema`。
    pub r#type: String,
    /// 调用方提供的 JSON Schema。
    pub schema: Map<String, Value>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 模型思考模式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thinking {
    /// `adaptive`、`enabled` 或 `disabled`。
    pub r#type: String,
    /// 旧版明确指定的思考词元预算。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub budget_tokens: OptionalNullable<u64>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 工具调用选择。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolChoice {
    /// `auto`、`any`、`tool` 或 `none`。
    pub r#type: String,
    /// 指定工具时的工具名。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub name: OptionalNullable<String>,
    /// 是否关闭并行工具调用。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub disable_parallel_tool_use: OptionalNullable<bool>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::Request;
    use serde_json::json;

    #[test]
    fn request_round_trip_with_cache_and_system_blocks() {
        let source = json!({
            "max_tokens":100,"messages":[{"role":"user","content":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral","ttl":"1h"}}]}],"model":"claude",
            "cache_control":{"type":"ephemeral","ttl":"5m"},
            "system":[{"type":"text","text":"You are helpful","cache_control":{"type":"ephemeral"}}],
            "thinking":{"type":"adaptive"},
            "output_config":{"effort":"medium","format":{"type":"json_schema","schema":{"type":"object"}}},
            "tools":[{"name":"lookup","input_schema":{"type":"object"}}],
            "stream":true,"vendor":1
        });
        let request: Request = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }
}
