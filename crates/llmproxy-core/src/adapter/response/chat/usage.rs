//! Chat 用量统计与通用 IR 的映射；流式最终用量分片复用同一映射。

use crate::{
    ir::{
        cache::CacheUsage,
        usage::{InputTokenDetails, OutputTokenDetails, Usage as IrUsage},
    },
    protocol::chat::{
        OptionalNullable,
        response::usage::{CompletionTokensDetails, PromptTokensDetails, Usage as ChatUsage},
    },
};

use super::super::{Error, Result};

/// 从 Chat 的 `usage` 中提取通用词元及缓存统计。
pub fn decode_chat_usage(usage: &ChatUsage) -> IrUsage {
    let input = match &usage.prompt_tokens_details {
        OptionalNullable::Value(details) => Some(details),
        _ => None,
    };
    let output = match &usage.completion_tokens_details {
        OptionalNullable::Value(details) => Some(details),
        _ => None,
    };
    IrUsage {
        input_tokens: Some(usage.prompt_tokens),
        output_tokens: Some(usage.completion_tokens),
        total_tokens: Some(usage.total_tokens),
        cache: CacheUsage {
            read_input_tokens: input.and_then(|details| present(&details.cached_tokens)),
            write_input_tokens: input.and_then(|details| present(&details.cache_write_tokens)),
        },
        input_details: InputTokenDetails {
            text_tokens: input.and_then(|details| present(&details.text_tokens)),
            audio_tokens: input.and_then(|details| present(&details.audio_tokens)),
            image_tokens: input.and_then(|details| present(&details.image_tokens)),
        },
        output_details: OutputTokenDetails {
            text_tokens: output.and_then(|details| present(&details.text_tokens)),
            audio_tokens: output.and_then(|details| present(&details.audio_tokens)),
            reasoning_tokens: output.and_then(|details| present(&details.reasoning_tokens)),
            accepted_prediction_tokens: output
                .and_then(|details| present(&details.accepted_prediction_tokens)),
            rejected_prediction_tokens: output
                .and_then(|details| present(&details.rejected_prediction_tokens)),
        },
    }
}

/// 将 IR 用量写入 Chat 结构；有原始结构时保留未映射的供应商字段。
pub fn encode_chat_usage(usage: &IrUsage, original: Option<&ChatUsage>) -> Result<ChatUsage> {
    let mut encoded = original.cloned().unwrap_or_else(|| ChatUsage {
        completion_tokens: 0,
        prompt_tokens: 0,
        total_tokens: 0,
        completion_tokens_details: OptionalNullable::Missing,
        prompt_tokens_details: OptionalNullable::Missing,
        extra: Default::default(),
    });
    encoded.prompt_tokens = required(
        usage.input_tokens,
        original.map(|value| value.prompt_tokens),
    )?;
    encoded.completion_tokens = required(
        usage.output_tokens,
        original.map(|value| value.completion_tokens),
    )?;
    encoded.total_tokens = required(usage.total_tokens, original.map(|value| value.total_tokens))?;

    if [
        usage.cache.read_input_tokens,
        usage.cache.write_input_tokens,
        usage.input_details.text_tokens,
        usage.input_details.audio_tokens,
        usage.input_details.image_tokens,
    ]
    .iter()
    .any(Option::is_some)
    {
        let details = get_input_details(&mut encoded.prompt_tokens_details);
        set(&mut details.cached_tokens, usage.cache.read_input_tokens);
        set(
            &mut details.cache_write_tokens,
            usage.cache.write_input_tokens,
        );
        set(&mut details.text_tokens, usage.input_details.text_tokens);
        set(&mut details.audio_tokens, usage.input_details.audio_tokens);
        set(&mut details.image_tokens, usage.input_details.image_tokens);
    }
    if [
        usage.output_details.text_tokens,
        usage.output_details.audio_tokens,
        usage.output_details.reasoning_tokens,
        usage.output_details.accepted_prediction_tokens,
        usage.output_details.rejected_prediction_tokens,
    ]
    .iter()
    .any(Option::is_some)
    {
        let details = get_output_details(&mut encoded.completion_tokens_details);
        set(&mut details.text_tokens, usage.output_details.text_tokens);
        set(&mut details.audio_tokens, usage.output_details.audio_tokens);
        set(
            &mut details.reasoning_tokens,
            usage.output_details.reasoning_tokens,
        );
        set(
            &mut details.accepted_prediction_tokens,
            usage.output_details.accepted_prediction_tokens,
        );
        set(
            &mut details.rejected_prediction_tokens,
            usage.output_details.rejected_prediction_tokens,
        );
    }
    Ok(encoded)
}

/// 只把协议实际提供的计数映射到 IR。
fn present(value: &OptionalNullable<u64>) -> Option<u64> {
    match value {
        OptionalNullable::Value(value) => Some(*value),
        _ => None,
    }
}

/// 构造完整 Chat 用量时，必需的总数不能凭空推断。
fn required(value: Option<u64>, original: Option<u64>) -> Result<u64> {
    value
        .or(original)
        .ok_or_else(|| Error::Unsupported("Chat usage 缺少必需的词元总数".into()))
}

/// 仅覆盖 IR 有值的计数，保留原协议未映射的细分统计。
fn set(target: &mut OptionalNullable<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *target = OptionalNullable::Value(value);
    }
}

fn get_input_details(
    value: &mut OptionalNullable<PromptTokensDetails>,
) -> &mut PromptTokensDetails {
    if !matches!(value, OptionalNullable::Value(_)) {
        *value = OptionalNullable::Value(PromptTokensDetails {
            audio_tokens: OptionalNullable::Missing,
            cache_write_tokens: OptionalNullable::Missing,
            cached_tokens: OptionalNullable::Missing,
            image_tokens: OptionalNullable::Missing,
            text_tokens: OptionalNullable::Missing,
            extra: Default::default(),
        });
    }
    let OptionalNullable::Value(details) = value else {
        unreachable!()
    };
    details
}

fn get_output_details(
    value: &mut OptionalNullable<CompletionTokensDetails>,
) -> &mut CompletionTokensDetails {
    if !matches!(value, OptionalNullable::Value(_)) {
        *value = OptionalNullable::Value(CompletionTokensDetails {
            accepted_prediction_tokens: OptionalNullable::Missing,
            audio_tokens: OptionalNullable::Missing,
            reasoning_tokens: OptionalNullable::Missing,
            rejected_prediction_tokens: OptionalNullable::Missing,
            text_tokens: OptionalNullable::Missing,
            extra: Default::default(),
        });
    }
    let OptionalNullable::Value(details) = value else {
        unreachable!()
    };
    details
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{decode_chat_usage, encode_chat_usage};
    use crate::protocol::chat::response::Usage;

    #[test]
    fn usage_maps_cache_and_preserves_unknown_details() {
        let source = json!({"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,"prompt_tokens_details":{"cached_tokens":60,"cache_write_tokens":10,"audio_tokens":2,"vendor":7},"completion_tokens_details":{"reasoning_tokens":5,"vendor":8},"vendor_usage":9});
        let raw: Usage = serde_json::from_value(source.clone()).unwrap();
        let mut ir = decode_chat_usage(&raw);
        assert_eq!(ir.cache.read_input_tokens, Some(60));
        assert_eq!(ir.cache.write_input_tokens, Some(10));
        assert_eq!(ir.output_details.reasoning_tokens, Some(5));
        assert_eq!(
            serde_json::to_value(encode_chat_usage(&ir, Some(&raw)).unwrap()).unwrap(),
            source
        );
        ir.cache.read_input_tokens = Some(70);
        let encoded = serde_json::to_value(encode_chat_usage(&ir, Some(&raw)).unwrap()).unwrap();
        assert_eq!(encoded["prompt_tokens_details"]["cached_tokens"], 70);
        assert_eq!(encoded["prompt_tokens_details"]["vendor"], 7);
    }

    #[test]
    fn usage_distinguishes_unreported_cache_from_zero() {
        let source = json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":0}});
        let raw: Usage = serde_json::from_value(source.clone()).unwrap();
        let ir = decode_chat_usage(&raw);
        assert_eq!(ir.cache.read_input_tokens, Some(0));
        assert_eq!(ir.cache.write_input_tokens, None);
        assert_eq!(
            serde_json::to_value(encode_chat_usage(&ir, Some(&raw)).unwrap()).unwrap(),
            source
        );
    }
}
