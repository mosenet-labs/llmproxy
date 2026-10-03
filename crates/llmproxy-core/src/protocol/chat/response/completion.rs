//! Chat Completions 的完整非流式响应。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::message::Message;
use super::{logprobs::Logprobs, moderation::Moderation, usage::Usage};
use crate::protocol::optional_nullable::OptionalNullable;

/// 一次非流式 Chat Completion 的完整响应外壳。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    /// 本次完成的唯一 ID。
    pub id: String,
    /// 各个候选回复，数量可能由请求的 `n` 决定。
    pub choices: Vec<Choice>,
    /// 创建时间的 Unix 秒级时间戳。
    pub created: i64,
    /// 实际生成回复的模型 ID。
    pub model: String,
    /// 对象类型，标准值为 `chat.completion`。
    pub object: String,
    /// 附加在完成对象上的键值元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub metadata: OptionalNullable<Map<String, Value>>,
    /// 请求与输出的审核结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub moderation: OptionalNullable<Moderation>,
    /// 实际处理请求的服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 旧版后端配置指纹。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub system_fingerprint: OptionalNullable<String>,
    /// 输入、输出和总词元用量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub usage: OptionalNullable<Usage>,
    /// 保留未声明的供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个非流式候选回复及其停止原因。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// 生成停止的原因，例如 `stop`、`length` 或 `tool_calls`。
    pub finish_reason: String,
    /// 候选回复在 `choices` 中的序号。
    pub index: u64,
    /// 词元对数概率；未请求时通常为 `null`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub logprobs: OptionalNullable<Logprobs>,
    /// 已生成完成的模型消息。
    pub message: Message,
    /// 保留未声明的候选字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Completion;

    #[test]
    fn completion_round_trip_with_multiple_choices_and_usage() {
        let source = json!({
            "id":"chatcmpl-1", "choices":[
                {"finish_reason":"stop","index":0,"logprobs":null,"message":{"role":"assistant","content":"你好"}},
                {"finish_reason":"tool_calls","index":1,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{}"}}]}}
            ],
            "created": 123, "model":"example-model", "object":"chat.completion",
            "usage":{"completion_tokens":2,"prompt_tokens":3,"total_tokens":5,"prompt_tokens_details":{"cached_tokens":1}},
            "service_tier":"default", "vendor_field":true
        });
        let completion: Completion = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(completion.choices.len(), 2);
        assert_eq!(serde_json::to_value(completion).unwrap(), source);
    }

    #[test]
    fn completion_round_trip_with_logprobs_moderation_and_citation() {
        let source = json!({
            "id":"chatcmpl-2","choices":[{"finish_reason":"stop","index":0,
                "logprobs":{"content":[{"token":"hi","bytes":[104,105],"logprob":-0.1,"top_logprobs":[{"token":"hey","bytes":[104,101,121],"logprob":-1.2}]}],"refusal":null},
                "message":{"role":"assistant","content":"hi","annotations":[{"type":"url_citation","url_citation":{"start_index":0,"end_index":2,"title":"Source","url":"https://example.com"}}]}}],
            "created":123,"model":"m","object":"chat.completion",
            "moderation":{"input":{"type":"moderation_results","model":"moderation-model","results":[{"categories":{"violence":false},"category_applied_input_types":{"violence":["text"]},"category_scores":{"violence":0.01},"flagged":false,"model":"moderation-model","type":"moderation_result"}]},"output":{"type":"error","code":"unavailable","message":"retry"}},
            "usage":{"completion_tokens":2,"prompt_tokens":3,"total_tokens":5,"completion_tokens_details":{"reasoning_tokens":1},"prompt_tokens_details":{"cached_tokens":1,"cache_write_tokens":2}}
        });
        let completion: Completion = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(completion).unwrap(), source);
    }
}
