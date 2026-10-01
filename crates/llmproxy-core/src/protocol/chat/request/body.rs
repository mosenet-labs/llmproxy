//! `POST /chat/completions` 的请求正文。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

use super::message::Message;
use super::parameters::{
    AudioOutput, FunctionChoice, FunctionDefinition, ModerationSettings, Prediction,
    PromptCacheOptions, ResponseFormat, StopSequences, Tool, ToolChoice, WebSearchOptions,
};
use crate::protocol::optional_nullable::OptionalNullable;

/// Chat Completions 请求；流式和非流式请求使用同一结构。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 到目前为止的对话消息。
    pub messages: Vec<Message>,
    /// 用于生成回复的模型 ID。
    pub model: String,
    /// 生成音频时使用的格式和声音设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio: OptionalNullable<AudioOutput>,
    /// 惩罚已在上下文中出现的词元，控制重复程度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub frequency_penalty: OptionalNullable<f64>,
    /// 旧版函数调用选择，现由 `tool_choice` 取代。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_call: OptionalNullable<FunctionChoice>,
    /// 旧版可调用函数列表，现由 `tools` 取代。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub functions: OptionalNullable<Vec<FunctionDefinition>>,
    /// 词元 ID 到偏置值的映射。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub logit_bias: OptionalNullable<BTreeMap<String, f64>>,
    /// 是否返回生成词元的对数概率。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub logprobs: OptionalNullable<bool>,
    /// 生成回复可使用的最大词元数，包含推理词元。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_completion_tokens: OptionalNullable<u64>,
    /// 旧版生成词元上限，现由 `max_completion_tokens` 取代。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub max_tokens: OptionalNullable<u64>,
    /// 附加在完成对象上的键值元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub metadata: OptionalNullable<Map<String, Value>>,
    /// 要生成的输出模态，例如文本或音频。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub modalities: OptionalNullable<Vec<String>>,
    /// 对输入和输出执行审核的设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub moderation: OptionalNullable<ModerationSettings>,
    /// 每条输入消息生成的候选回复数量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub n: OptionalNullable<u64>,
    /// 是否允许并行调用多个工具。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parallel_tool_calls: OptionalNullable<bool>,
    /// 已知输出内容，用于预测式生成。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prediction: OptionalNullable<Prediction>,
    /// 惩罚此前出现过的词元，鼓励引入新话题。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub presence_penalty: OptionalNullable<f64>,
    /// 用于相似请求缓存匹配的键。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_key: OptionalNullable<String>,
    /// 提示缓存的断点模式和生存时间设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_options: OptionalNullable<PromptCacheOptions>,
    /// 旧版提示缓存保留策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub prompt_cache_retention: OptionalNullable<String>,
    /// 推理模型使用的推理强度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub reasoning_effort: OptionalNullable<String>,
    /// 文本、JSON 对象或 JSON Schema 等回复格式约束。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub response_format: OptionalNullable<ResponseFormat>,
    /// 用于识别终端用户的安全标识。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub safety_identifier: OptionalNullable<String>,
    /// 旧版确定性采样种子。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub seed: OptionalNullable<i64>,
    /// 请求使用的服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 一个停止序列或多个停止序列。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stop: OptionalNullable<StopSequences>,
    /// 是否保存本次完成结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub store: OptionalNullable<bool>,
    /// 是否以 SSE 分片返回回复。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stream: OptionalNullable<bool>,
    /// 流式响应的用量和混淆设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub stream_options: OptionalNullable<StreamOptions>,
    /// 采样温度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub temperature: OptionalNullable<f64>,
    /// 不调用、自动调用、强制调用或指定工具的选择。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_choice: OptionalNullable<ToolChoice>,
    /// 模型可以调用的函数工具或自定义工具。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tools: OptionalNullable<Vec<Tool>>,
    /// 每个词元最多返回的候选对数概率数量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_logprobs: OptionalNullable<u64>,
    /// 核采样阈值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub top_p: OptionalNullable<f64>,
    /// 旧版终端用户标识。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub user: OptionalNullable<String>,
    /// 输出详细程度。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub verbosity: OptionalNullable<String>,
    /// 网页搜索工具的搜索范围及用户位置设置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub web_search_options: OptionalNullable<WebSearchOptions>,
    /// 保留未声明的供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 仅在 `stream: true` 时适用的流式选项。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamOptions {
    /// 是否给流式分片加入混淆字段。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub include_obfuscation: OptionalNullable<bool>,
    /// 是否在结束标记前附加用量分片。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub include_usage: OptionalNullable<bool>,
    /// 保留未声明的流式选项。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Request;
    use crate::protocol::optional_nullable::OptionalNullable;

    #[test]
    fn request_round_trip_with_stream_and_extensions() {
        let source = json!({
            "messages": [{"role":"user","content":"你好"}],
            "model": "example-model",
            "stream": true,
            "stream_options": {"include_usage": true, "vendor_option": 1},
            "tools": [{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}],
            "tool_choice": "auto",
            "response_format": {"type":"json_object"},
            "temperature": null,
            "vendor_field": {"enabled": true}
        });
        let request: Request = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(request.stream, OptionalNullable::Value(true));
        assert_eq!(request.temperature, OptionalNullable::Null);
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }

    #[test]
    fn request_requires_model_and_messages() {
        let minimal = json!({"messages":[{"role":"user","content":"hi"}],"model":"m"});
        let request: Request = serde_json::from_value(minimal.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), minimal);
        assert!(serde_json::from_value::<Request>(json!({"model":"m"})).is_err());
        assert!(serde_json::from_value::<Request>(json!({"messages":[]})).is_err());
    }

    #[test]
    fn request_round_trip_with_structured_options() {
        let source = json!({
            "messages": [{"role":"user","content":[{"type":"text","text":"hello","prompt_cache_breakpoint":{"mode":"explicit"}}]}],
            "model":"m",
            "audio":{"format":"wav","voice":{"id":"voice_1"}},
            "function_call":{"name":"old_lookup"},
            "functions":[{"name":"old_lookup","parameters":{"type":"object"}}],
            "moderation":{"model":"omni-moderation-latest","policy":{"input":{"mode":"block"},"output":{"mode":"score"}}},
            "prediction":{"type":"content","content":[{"type":"text","text":"known"}]},
            "prompt_cache_options":{"mode":"explicit","ttl":"30m"},
            "response_format":{"type":"json_schema","json_schema":{"name":"answer","strict":true,"schema":{"type":"object"}}},
            "stop":["END"],
            "tool_choice":{"type":"allowed_tools","allowed_tools":{"mode":"auto","tools":[{"type":"function","function":{"name":"lookup"}}]}},
            "tools":[{"type":"custom","custom":{"name":"execute","format":{"type":"grammar","grammar":{"definition":"start: WORD","syntax":"lark"}}}}],
            "web_search_options":{"search_context_size":"medium","user_location":{"type":"approximate","approximate":{"city":"Shanghai","country":"CN"}}}
        });
        let request: Request = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }
}
