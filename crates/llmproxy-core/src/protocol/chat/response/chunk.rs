//! Chat Completions 的流式响应分片；一个 SSE `data` 对应一个 `Chunk`。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{logprobs::Logprobs, moderation::Moderation, usage::Usage};
use crate::protocol::optional_nullable::OptionalNullable;

/// 流式响应中的单个 `chat.completion.chunk` JSON 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    /// 完成 ID，同一响应的各分片使用相同 ID。
    pub id: String,
    /// 候选回复的增量；最终用量分片可以为空数组。
    pub choices: Vec<Choice>,
    /// 完成创建时间的 Unix 秒级时间戳。
    pub created: i64,
    /// 用于生成回复的模型 ID。
    pub model: String,
    /// 对象类型，标准值为 `chat.completion.chunk`。
    pub object: String,
    /// 启用审核时在专门分片中返回的结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub moderation: OptionalNullable<Moderation>,
    /// 平衡分片大小的混淆字符串。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub obfuscation: OptionalNullable<String>,
    /// 实际处理请求的服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 旧版后端配置指纹。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub system_fingerprint: OptionalNullable<String>,
    /// 词元用量；启用 `include_usage` 时通常只在最后一个分片中非空。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub usage: OptionalNullable<Usage>,
    /// 保留未声明的供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个候选回复在当前分片中的变化。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// 本分片新增的角色、文本、拒绝或工具调用片段。
    pub delta: Delta,
    /// 候选回复的停止原因；生成尚未完成时通常为 `null`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub finish_reason: OptionalNullable<String>,
    /// 候选回复序号，用于组合不同分片。
    pub index: u64,
    /// 本分片词元的对数概率。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub logprobs: OptionalNullable<Logprobs>,
    /// 保留未声明的候选字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 流式消息增量；所有字段都可能只在部分分片中出现。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    /// 流式音频的 ID、数据、转录或到期时间可分多次返回。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio: OptionalNullable<AudioDelta>,
    /// 当前分片的文本内容。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub content: OptionalNullable<String>,
    /// 旧版函数调用的增量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_call: OptionalNullable<FunctionCallDelta>,
    /// 当前分片的拒绝文本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub refusal: OptionalNullable<String>,
    /// 消息角色，通常只在首个分片出现。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub role: OptionalNullable<String>,
    /// 工具调用增量；通过各项 `index` 组合。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_calls: OptionalNullable<Vec<ToolCallDelta>>,
    /// 保留未声明的增量字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 音频的局部更新，不能直接复用非流式的完整 Audio。
/// 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioDelta {
    /// 本分片的 Base64 音频数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub data: OptionalNullable<String>,
    /// 音频资源到期的 Unix 秒级时间戳，通常在最后一次更新出现。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub expires_at: OptionalNullable<i64>,
    /// 音频标识，可能只在首个分片出现。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 当前分片的转录文本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub transcript: OptionalNullable<String>,
    /// 保留未声明的音频扩展。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 同一个工具调用在当前分片中新增的字段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallDelta {
    /// 工具调用在当前候选回复中的序号。
    pub index: u64,
    /// 工具调用 ID，可能只在首个相关分片出现。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 函数名与参数的增量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function: OptionalNullable<FunctionCallDelta>,
    /// 工具类型，可能只在首个相关分片出现。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub r#type: OptionalNullable<String>,
    /// 保留未声明的工具调用增量字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 函数名与参数字符串的局部增量，不能按完整函数调用解析。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCallDelta {
    /// 本分片新增的函数参数字符串。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub arguments: OptionalNullable<String>,
    /// 函数名，可能只在首个相关分片出现。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub name: OptionalNullable<String>,
    /// 保留未声明的函数调用增量字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Chunk;
    use crate::protocol::optional_nullable::OptionalNullable;

    #[test]
    fn chunks_round_trip_partial_calls_and_final_usage() {
        let chunks = [
            json!({"id":"c1","choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":""}}]},"finish_reason":null,"index":0,"logprobs":null}],"created":123,"model":"m","object":"chat.completion.chunk"}),
            json!({"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"key\":1}"}}]},"finish_reason":"tool_calls","index":0}],"created":123,"model":"m","object":"chat.completion.chunk"}),
            json!({"id":"c1","choices":[],"created":123,"model":"m","object":"chat.completion.chunk","usage":{"completion_tokens":2,"prompt_tokens":3,"total_tokens":5}}),
        ];
        for source in chunks {
            let chunk: Chunk = serde_json::from_value(source.clone()).unwrap();
            assert_eq!(serde_json::to_value(chunk).unwrap(), source);
        }
    }

    #[test]
    fn chunk_keeps_missing_and_null_usage_distinct() {
        let base = json!({"id":"c1","choices":[],"created":123,"model":"m","object":"chat.completion.chunk"});
        let missing: Chunk = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(missing.usage, OptionalNullable::Missing);
        let mut null = base;
        null["usage"] = json!(null);
        let parsed: Chunk = serde_json::from_value(null.clone()).unwrap();
        assert_eq!(parsed.usage, OptionalNullable::Null);
        assert_eq!(serde_json::to_value(parsed).unwrap(), null);
    }

    #[test]
    fn audio_final_update_does_not_require_full_audio_or_finish_reason() {
        for delta in [
            json!({"audio":{"id":"a","data":"YQ==","transcript":"Hi"}}),
            json!({"audio":{"expires_at":123}}),
            json!({"audio":null}),
        ] {
            let source = json!({"id":"c","choices":[{"index":0,"delta":delta}],"created":1,"model":"m","object":"chat.completion.chunk"});
            let chunk: Chunk =
                serde_json::from_slice(&serde_json::to_vec(&source).unwrap()).unwrap();
            assert_eq!(serde_json::to_value(chunk).unwrap(), source);
        }
    }
}
