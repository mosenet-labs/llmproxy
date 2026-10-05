//! 探测成功响应复用 Core 的用量口径，局部兼容缺失总数的供应商接口。
use super::TokenUsage;
use llmproxy_core::{adapter::response, protocol::Protocol};
use serde_json::Value;

/// 只解析用量叶子，避免对探测响应的其他字段施加完整协议校验。
pub(super) fn decode(protocol: Protocol, body: &Value) -> Option<TokenUsage> {
    let usage = match protocol {
        Protocol::OpenAiChat => response::decode_chat_usage(
            &serde_json::from_value(with_total(
                &body["usage"],
                "prompt_tokens",
                "completion_tokens",
            )?)
            .ok()?,
        ),
        Protocol::OpenAiResponses => response::decode_responses_usage(
            &serde_json::from_value(with_total(&body["usage"], "input_tokens", "output_tokens")?)
                .ok()?,
        ),
        Protocol::AnthropicMessages => response::decode_messages_usage(
            &serde_json::from_value(body.get("usage")?.clone()).ok()?,
        ),
        Protocol::Gemini => response::decode_gemini_usage(
            &serde_json::from_value(body.get("usageMetadata")?.clone()).ok()?,
        ),
    };
    Some(TokenUsage {
        input: usage.input_tokens?,
        output: usage.output_tokens?,
    })
}

/// 兼容接口仅报告输入和输出时，在探测边界补出可验证总数；不补零计数。
fn with_total(value: &Value, input: &str, output: &str) -> Option<Value> {
    let mut fields = value.as_object()?.clone();
    if !fields.contains_key("total_tokens") {
        let total = fields
            .get(input)?
            .as_u64()?
            .checked_add(fields.get(output)?.as_u64()?)?;
        fields.insert("total_tokens".into(), total.into());
    }
    Some(Value::Object(fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn probe_usage_uses_cache_and_reasoning_totals() {
        for (protocol, body, input, output) in [
            (
                Protocol::OpenAiChat,
                json!({"usage":{"prompt_tokens":8,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":4}}}),
                8,
                1,
            ),
            (
                Protocol::OpenAiResponses,
                json!({"usage":{"input_tokens":8,"output_tokens":1}}),
                8,
                1,
            ),
            (
                Protocol::AnthropicMessages,
                json!({"usage":{"input_tokens":20,"output_tokens":5,"cache_read_input_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20}}}),
                100,
                5,
            ),
            (
                Protocol::Gemini,
                json!({"usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":1,"thoughtsTokenCount":2}}),
                8,
                3,
            ),
        ] {
            assert_eq!(decode(protocol, &body), Some(TokenUsage { input, output }));
        }
    }

    #[test]
    fn absent_invalid_and_overflowed_counts_are_not_reported_as_zero() {
        for protocol in [
            Protocol::OpenAiChat,
            Protocol::OpenAiResponses,
            Protocol::AnthropicMessages,
            Protocol::Gemini,
        ] {
            assert_eq!(decode(protocol, &json!({})), None);
        }
        assert_eq!(
            decode(
                Protocol::OpenAiChat,
                &json!({"usage":{"prompt_tokens":u64::MAX,"completion_tokens":1}})
            ),
            None
        );
        assert_eq!(
            decode(
                Protocol::AnthropicMessages,
                &json!({"usage":{"input_tokens":u64::MAX,"output_tokens":1,"cache_read_input_tokens":1}})
            ),
            None
        );
        assert_eq!(
            decode(
                Protocol::Gemini,
                &json!({"usageMetadata":{"promptTokenCount":0,"candidatesTokenCount":0}})
            ),
            Some(TokenUsage {
                input: 0,
                output: 0
            })
        );
    }
}
