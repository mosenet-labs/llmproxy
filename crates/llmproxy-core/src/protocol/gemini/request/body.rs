//! Gemini `generateContent` 与 `streamGenerateContent` 的请求正文。
//! 参考 API：https://ai.google.dev/api/generate-content

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::message::Message;
use crate::protocol::OptionalNullable;

/// Gemini 生成请求；模型 ID 位于 URL 路径，不在请求正文中。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// 对话历史和当前输入。
    pub contents: Vec<Message>,
    /// 函数声明、搜索等工具。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tools: OptionalNullable<Vec<Tool>>,
    /// 工具调用约束。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_config: OptionalNullable<ToolConfig>,
    /// 内容安全过滤策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub safety_settings: OptionalNullable<Vec<SafetySetting>>,
    /// 请求标签。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub labels: OptionalNullable<Map<String, Value>>,
    /// 系统指令。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub system_instruction: OptionalNullable<Message>,
    /// 生成与输出格式配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub generation_config: OptionalNullable<GenerationConfig>,
    /// 显式缓存内容引用。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cached_content: OptionalNullable<String>,
    /// 服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 是否保存生成日志。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub store: OptionalNullable<bool>,
    /// 保留新增请求字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 可选的函数、搜索、代码执行等工具集合。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// 客户端可执行的函数声明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_declarations: OptionalNullable<Vec<FunctionDeclaration>>,
    /// 旧版动态 Google 搜索配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub google_search_retrieval: OptionalNullable<Map<String, Value>>,
    /// 代码执行配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub code_execution: OptionalNullable<Map<String, Value>>,
    /// Google 搜索配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub google_search: OptionalNullable<Map<String, Value>>,
    /// 计算机操作配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub computer_use: OptionalNullable<Map<String, Value>>,
    /// URL 上下文配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub url_context: OptionalNullable<Map<String, Value>>,
    /// 文件搜索配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub file_search: OptionalNullable<Map<String, Value>>,
    /// MCP 服务器配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub mcp_servers: OptionalNullable<Vec<Map<String, Value>>>,
    /// 地图工具配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub google_maps: OptionalNullable<Map<String, Value>>,
    /// 保留新增工具类型。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 模型可以请求客户端执行的函数。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    /// 函数名称。
    pub name: String,
    /// 函数用途说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 函数调用行为。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub behavior: OptionalNullable<String>,
    /// OpenAPI 参数结构。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parameters: OptionalNullable<Map<String, Value>>,
    /// JSON Schema 参数结构。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parameters_json_schema: OptionalNullable<Value>,
    /// OpenAPI 结果结构。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response: OptionalNullable<Map<String, Value>>,
    /// JSON Schema 结果结构。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_json_schema: OptionalNullable<Value>,
    /// 保留新增函数字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 工具调用约束。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    /// 函数调用模式与允许的函数名。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_calling_config: OptionalNullable<FunctionCallingConfig>,
    /// 保留新增工具约束。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 函数调用模式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCallingConfig {
    /// `AUTO`、`ANY`、`NONE` 等模式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub mode: OptionalNullable<String>,
    /// 允许模型调用的函数名。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub allowed_function_names: OptionalNullable<Vec<String>>,
    /// 保留新增约束。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 某一安全类别的过滤阈值。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SafetySetting {
    /// 内容安全类别。
    pub category: String,
    /// 阻断阈值。
    pub threshold: String,
    /// 保留新增安全字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 生成策略及响应格式配置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    /// 生成停止序列。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop_sequences: OptionalNullable<Vec<String>>,
    /// 输出 MIME 类型。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_mime_type: OptionalNullable<String>,
    /// 旧版 OpenAPI 输出结构。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_schema: OptionalNullable<Map<String, Value>>,
    /// 旧版 JSON Schema 输出结构。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_json_schema: OptionalNullable<Value>,
    /// 允许的输出模态。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_modalities: OptionalNullable<Vec<String>>,
    /// 候选回复数量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub candidate_count: OptionalNullable<u64>,
    /// 最大输出词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_output_tokens: OptionalNullable<u64>,
    /// 采样温度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub temperature: OptionalNullable<f64>,
    /// 核采样阈值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_p: OptionalNullable<f64>,
    /// 候选词元的最大数量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_k: OptionalNullable<u64>,
    /// 随机种子。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub seed: OptionalNullable<i64>,
    /// 话题重复惩罚。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub presence_penalty: OptionalNullable<f64>,
    /// 词元重复惩罚。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub frequency_penalty: OptionalNullable<f64>,
    /// 是否返回生成词元的对数概率。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_logprobs: OptionalNullable<bool>,
    /// 返回的候选对数概率数量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub logprobs: OptionalNullable<u64>,
    /// 是否启用增强公民事务回答。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub enable_enhanced_civic_answers: OptionalNullable<bool>,
    /// 语音生成配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub speech_config: OptionalNullable<Map<String, Value>>,
    /// 思考预算和级别。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thinking_config: OptionalNullable<ThinkingConfig>,
    /// 图片生成配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub image_config: OptionalNullable<Map<String, Value>>,
    /// 多模态处理分辨率。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub media_resolution: OptionalNullable<String>,
    /// 是否启用情感对话。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub enable_affective_dialog: OptionalNullable<bool>,
    /// 新版输出格式设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_format: OptionalNullable<Map<String, Value>>,
    /// 翻译配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub translation_config: OptionalNullable<Map<String, Value>>,
    /// 音频转录配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio_transcription_config: OptionalNullable<Map<String, Value>>,
    /// 保留新增生成参数。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 模型思考输出设置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// 是否在响应中包含思考内容。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub include_thoughts: OptionalNullable<bool>,
    /// 最大思考词元预算。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thinking_budget: OptionalNullable<i64>,
    /// 思考强度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thinking_level: OptionalNullable<String>,
    /// 保留新增字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::Request;
    use serde_json::json;

    #[test]
    fn request_round_trip_with_cached_content_and_generation_config() {
        let source = json!({
            "contents":[{"role":"user","parts":[{"text":"hi"}]}],
            "tools":[{"functionDeclarations":[{"name":"lookup","description":"Find","parameters":{"type":"OBJECT"}}],"googleSearch":{}}],
            "toolConfig":{"functionCallingConfig":{"mode":"AUTO","allowedFunctionNames":["lookup"]}},
            "generationConfig":{"maxOutputTokens":100,"responseMimeType":"application/json","responseJsonSchema":{"type":"object"},"thinkingConfig":{"thinkingLevel":"LOW"}},
            "cachedContent":"cachedContents/1","serviceTier":"STANDARD","labels":{"source":"test"}
        });
        let request: Request = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }
}
