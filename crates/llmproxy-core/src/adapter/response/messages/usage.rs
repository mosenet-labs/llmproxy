//! Messages 缓存词元与 IR 用量的双向映射。

use crate::{
    adapter::nullable::{present, set},
    ir::{
        cache::CacheUsage,
        usage::{InputTokenDetails, OutputTokenDetails, Usage as IrUsage},
    },
    protocol::{
        OptionalNullable,
        messages::response::usage::{CacheCreation, OutputTokensDetails, Usage as RawUsage},
    },
};

use super::super::{Error, Result};

/// 将未缓存输入、缓存读写与输出分别记录；总输入量包含三部分。
pub fn decode_messages_usage(usage: &RawUsage) -> IrUsage {
    let read = present(&usage.cache_read_input_tokens);
    let creation = match &usage.cache_creation {
        OptionalNullable::Value(creation) => Some(creation),
        _ => None,
    };
    let write = present(&usage.cache_creation_input_tokens).or_else(|| {
        creation.and_then(|value| {
            let short = present(&value.ephemeral_5m_input_tokens);
            let long = present(&value.ephemeral_1h_input_tokens);
            (short.is_some() || long.is_some())
                .then(|| short.unwrap_or(0).checked_add(long.unwrap_or(0)))
                .flatten()
        })
    });
    let output = match &usage.output_tokens_details {
        OptionalNullable::Value(output) => Some(output),
        _ => None,
    };
    let input_total = usage
        .input_tokens
        .checked_add(read.unwrap_or(0))
        .and_then(|count| count.checked_add(write.unwrap_or(0)));
    IrUsage {
        input_tokens: input_total,
        output_tokens: Some(usage.output_tokens),
        total_tokens: input_total.and_then(|count| count.checked_add(usage.output_tokens)),
        cache: CacheUsage {
            read_input_tokens: read,
            write_input_tokens: write,
            write_short_input_tokens: creation
                .and_then(|value| present(&value.ephemeral_5m_input_tokens)),
            write_long_input_tokens: creation
                .and_then(|value| present(&value.ephemeral_1h_input_tokens)),
        },
        input_details: InputTokenDetails {
            uncached_tokens: Some(usage.input_tokens),
            ..Default::default()
        },
        output_details: OutputTokenDetails {
            reasoning_tokens: output.and_then(|value| present(&value.thinking_tokens)),
            ..Default::default()
        },
    }
}

/// 将 IR 用量写回 Messages；优先使用明确的未缓存输入计数。
pub fn encode_messages_usage(usage: &IrUsage, original: Option<&RawUsage>) -> Result<RawUsage> {
    let read = usage
        .cache
        .read_input_tokens
        .or(original.and_then(|value| present(&value.cache_read_input_tokens)));
    let write = usage
        .cache
        .write_input_tokens
        .or(original.and_then(|value| present(&value.cache_creation_input_tokens)))
        .or_else(|| {
            let short = usage.cache.write_short_input_tokens;
            let long = usage.cache.write_long_input_tokens;
            (short.is_some() || long.is_some())
                .then(|| short.unwrap_or(0).checked_add(long.unwrap_or(0)))
                .flatten()
        });
    let uncached = usage
        .input_details
        .uncached_tokens
        .or_else(|| {
            usage.input_tokens.and_then(|count| {
                count
                    .checked_sub(read.unwrap_or(0))
                    .and_then(|count| count.checked_sub(write.unwrap_or(0)))
            })
        })
        .or(original.map(|value| value.input_tokens))
        .ok_or_else(|| Error::Unsupported("Messages usage 缺少未缓存输入词元数".into()))?;
    let output = usage
        .output_tokens
        .or(original.map(|value| value.output_tokens))
        .ok_or_else(|| Error::Unsupported("Messages usage 缺少输出词元数".into()))?;
    let mut encoded = original.cloned().unwrap_or(RawUsage {
        cache_creation: OptionalNullable::Missing,
        cache_creation_input_tokens: OptionalNullable::Missing,
        cache_read_input_tokens: OptionalNullable::Missing,
        inference_geo: OptionalNullable::Missing,
        input_tokens: uncached,
        output_tokens: output,
        output_tokens_details: OptionalNullable::Missing,
        server_tool_use: OptionalNullable::Missing,
        service_tier: OptionalNullable::Missing,
        extra: Default::default(),
    });
    encoded.input_tokens = uncached;
    encoded.output_tokens = output;
    if let Some(read) = usage.cache.read_input_tokens {
        encoded.cache_read_input_tokens = OptionalNullable::Value(read);
    }
    let original_write = original
        .map(decode_messages_usage)
        .and_then(|value| value.cache.write_input_tokens);
    if let Some(write) = write.filter(|write| {
        original.is_none()
            || original.is_some_and(|value| present(&value.cache_creation_input_tokens).is_some())
            || original_write != Some(*write)
    }) {
        encoded.cache_creation_input_tokens = OptionalNullable::Value(write);
    }
    if usage.cache.write_short_input_tokens.is_some()
        || usage.cache.write_long_input_tokens.is_some()
    {
        if !matches!(encoded.cache_creation, OptionalNullable::Value(_)) {
            encoded.cache_creation = OptionalNullable::Value(CacheCreation::default());
        }
        if let OptionalNullable::Value(creation) = &mut encoded.cache_creation {
            set(
                &mut creation.ephemeral_5m_input_tokens,
                usage.cache.write_short_input_tokens,
            );
            set(
                &mut creation.ephemeral_1h_input_tokens,
                usage.cache.write_long_input_tokens,
            );
        }
    }
    if let Some(reasoning) = usage.output_details.reasoning_tokens {
        if !matches!(encoded.output_tokens_details, OptionalNullable::Value(_)) {
            encoded.output_tokens_details = OptionalNullable::Value(OutputTokensDetails::default());
        }
        if let OptionalNullable::Value(details) = &mut encoded.output_tokens_details {
            details.thinking_tokens = OptionalNullable::Value(reasoning);
        }
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::{decode_messages_usage, encode_messages_usage};
    use crate::protocol::messages::response::usage::Usage;
    use serde_json::json;

    #[test]
    fn input_total_includes_cache_without_double_counting() {
        let source = json!({"input_tokens":20,"output_tokens":5,"cache_read_input_tokens":50,"cache_creation_input_tokens":30,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20,"vendor":1},"output_tokens_details":{"thinking_tokens":2},"service_tier":"standard"});
        let raw: Usage = serde_json::from_value(source.clone()).unwrap();
        let mut ir = decode_messages_usage(&raw);
        assert_eq!(ir.input_tokens, Some(100));
        assert_eq!(ir.input_details.uncached_tokens, Some(20));
        assert_eq!(ir.total_tokens, Some(105));
        assert_eq!(ir.cache.write_short_input_tokens, Some(10));
        assert_eq!(
            serde_json::to_value(encode_messages_usage(&ir, Some(&raw)).unwrap()).unwrap(),
            source
        );
        ir.input_details.uncached_tokens = Some(25);
        let encoded =
            serde_json::to_value(encode_messages_usage(&ir, Some(&raw)).unwrap()).unwrap();
        assert_eq!(encoded["input_tokens"], 25);
        assert_eq!(encoded["cache_creation"]["vendor"], 1);
    }

    #[test]
    fn ttl_breakdown_can_supply_missing_cache_write_total() {
        let raw: Usage = serde_json::from_value(json!({
            "input_tokens": 20,
            "output_tokens": 5,
            "cache_creation": {"ephemeral_5m_input_tokens": 10, "ephemeral_1h_input_tokens": 20}
        }))
        .unwrap();
        let ir = decode_messages_usage(&raw);
        assert_eq!(ir.cache.write_input_tokens, Some(30));
        assert_eq!(ir.input_tokens, Some(50));
        assert_eq!(encode_messages_usage(&ir, Some(&raw)).unwrap(), raw);
        let encoded = encode_messages_usage(&ir, None).unwrap();
        assert_eq!(
            encoded.cache_creation_input_tokens,
            crate::protocol::OptionalNullable::Value(30)
        );
    }
}
