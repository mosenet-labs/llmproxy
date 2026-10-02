//! Responses `usage` 与通用词元统计的双向映射。

use crate::{
    adapter::nullable::{present, set},
    ir::{
        cache::CacheUsage,
        usage::{InputTokenDetails, OutputTokenDetails, Usage as IrUsage},
    },
    protocol::{
        OptionalNullable,
        responses::response::usage::{InputTokensDetails, OutputTokensDetails, Usage as RawUsage},
    },
};

use super::super::{Error, Result};

/// 提取 Responses 的输入、输出、缓存和推理计数。
pub fn decode_responses_usage(usage: &RawUsage) -> IrUsage {
    let input = match &usage.input_tokens_details {
        OptionalNullable::Value(details) => Some(details),
        _ => None,
    };
    let output = match &usage.output_tokens_details {
        OptionalNullable::Value(details) => Some(details),
        _ => None,
    };
    let read = input.and_then(|details| present(&details.cached_tokens));
    let write = input.and_then(|details| present(&details.cache_write_tokens));
    IrUsage {
        input_tokens: Some(usage.input_tokens),
        output_tokens: Some(usage.output_tokens),
        total_tokens: Some(usage.total_tokens),
        cache: CacheUsage {
            read_input_tokens: read,
            write_input_tokens: write,
            ..Default::default()
        },
        input_details: InputTokenDetails {
            uncached_tokens: read
                .zip(write)
                .and_then(|(read, write)| usage.input_tokens.checked_sub(read)?.checked_sub(write)),
            ..Default::default()
        },
        output_details: OutputTokenDetails {
            reasoning_tokens: output.and_then(|details| present(&details.reasoning_tokens)),
            ..Default::default()
        },
    }
}

/// 只覆盖 IR 中有值的计数；原始用量里的新增字段保持不变。
pub fn encode_responses_usage(usage: &IrUsage, original: Option<&RawUsage>) -> Result<RawUsage> {
    let input_tokens = usage
        .input_tokens
        .or(original.map(|value| value.input_tokens))
        .ok_or_else(|| Error::Unsupported("Responses usage 缺少输入词元数".into()))?;
    let output_tokens = usage
        .output_tokens
        .or(original.map(|value| value.output_tokens))
        .ok_or_else(|| Error::Unsupported("Responses usage 缺少输出词元数".into()))?;
    let total_tokens = usage
        .total_tokens
        .or(original.map(|value| value.total_tokens))
        .ok_or_else(|| Error::Unsupported("Responses usage 缺少总词元数".into()))?;
    let mut encoded = original.cloned().unwrap_or(RawUsage {
        input_tokens,
        input_tokens_details: OptionalNullable::Missing,
        output_tokens,
        output_tokens_details: OptionalNullable::Missing,
        total_tokens,
        extra: Default::default(),
    });
    encoded.input_tokens = input_tokens;
    encoded.output_tokens = output_tokens;
    encoded.total_tokens = total_tokens;
    if usage.cache.read_input_tokens.is_some() || usage.cache.write_input_tokens.is_some() {
        if !matches!(encoded.input_tokens_details, OptionalNullable::Value(_)) {
            encoded.input_tokens_details = OptionalNullable::Value(InputTokensDetails::default());
        }
        if let OptionalNullable::Value(details) = &mut encoded.input_tokens_details {
            set(&mut details.cached_tokens, usage.cache.read_input_tokens);
            set(
                &mut details.cache_write_tokens,
                usage.cache.write_input_tokens,
            );
        }
    }
    if let Some(reasoning) = usage.output_details.reasoning_tokens {
        if !matches!(encoded.output_tokens_details, OptionalNullable::Value(_)) {
            encoded.output_tokens_details = OptionalNullable::Value(OutputTokensDetails::default());
        }
        if let OptionalNullable::Value(details) = &mut encoded.output_tokens_details {
            details.reasoning_tokens = OptionalNullable::Value(reasoning);
        }
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::{decode_responses_usage, encode_responses_usage};
    use crate::protocol::responses::response::usage::Usage;
    use serde_json::json;

    #[test]
    fn usage_maps_cache_and_reasoning_without_losing_extensions() {
        let source = json!({"input_tokens":10,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":4,"vendor":7},"output_tokens":3,"output_tokens_details":{"reasoning_tokens":2},"total_tokens":13,"vendor_usage":true});
        let raw: Usage = serde_json::from_value(source.clone()).unwrap();
        let mut ir = decode_responses_usage(&raw);
        assert_eq!(ir.cache.read_input_tokens, Some(0));
        assert_eq!(ir.cache.write_input_tokens, Some(4));
        assert_eq!(ir.input_details.uncached_tokens, Some(6));
        assert_eq!(
            serde_json::to_value(encode_responses_usage(&ir, Some(&raw)).unwrap()).unwrap(),
            source
        );
        ir.cache.read_input_tokens = Some(5);
        let encoded =
            serde_json::to_value(encode_responses_usage(&ir, Some(&raw)).unwrap()).unwrap();
        assert_eq!(encoded["input_tokens_details"]["cached_tokens"], 5);
        assert_eq!(encoded["input_tokens_details"]["vendor"], 7);
    }
}
