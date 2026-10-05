//! 跨协议用量只丢弃目标不能表达的细分，总数和缓存读写不互相覆盖。
//! 参考：https://ai.google.dev/api/generate-content#UsageMetadata
//! 参考：https://platform.claude.com/docs/en/build-with-claude/prompt-caching
use super::{ConversionWarning, unsupported, warn};
use crate::{adapter::Result, ir::usage::Usage, protocol::Protocol};

/// 保留可映射计数；总数缺失时仅由已知输入、输出推导，绝不补造零用量。
pub(in crate::adapter::protocol_codec) fn normalize(
    usage: &Usage,
    source: Protocol,
    target: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<Usage> {
    let mut usage = usage.clone();
    if let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) {
        let total = input
            .checked_add(output)
            .ok_or_else(|| unsupported("usage.total_tokens", "词元总数溢出"))?;
        if usage.total_tokens.is_some_and(|reported| reported != total) {
            return Err(unsupported("usage.total_tokens", "与输入输出词元数不一致"));
        }
        usage.total_tokens = Some(total);
    }
    let cache = &mut usage.cache;
    if cache.write_input_tokens.is_none()
        && (cache.write_short_input_tokens.is_some() || cache.write_long_input_tokens.is_some())
    {
        cache.write_input_tokens = Some(
            cache
                .write_short_input_tokens
                .unwrap_or(0)
                .checked_add(cache.write_long_input_tokens.unwrap_or(0))
                .ok_or_else(|| unsupported("usage.cache", "缓存写入词元数溢出"))?,
        );
    }
    if let Some(input) = usage.input_tokens {
        let uncached = input
            .checked_sub(cache.read_input_tokens.unwrap_or(0))
            .and_then(|n| n.checked_sub(cache.write_input_tokens.unwrap_or(0)))
            .ok_or_else(|| unsupported("usage.cache", "缓存读写超过输入总数"))?;
        if usage
            .input_details
            .uncached_tokens
            .is_some_and(|n| n != uncached)
        {
            return Err(unsupported(
                "usage.input_details.uncached_tokens",
                "与缓存读写及输入总数不一致",
            ));
        }
    }
    if usage
        .output_details
        .reasoning_tokens
        .zip(usage.output_tokens)
        .is_some_and(|(a, b)| a > b)
    {
        return Err(unsupported(
            "usage.output_details.reasoning_tokens",
            "推理词元超过输出总数",
        ));
    }
    macro_rules! drop_count {
        ($value:expr, $path:literal) => {
            if $value.take().is_some() {
                warn(
                    warnings,
                    source,
                    target,
                    $path,
                    "目标协议没有对应的用量细分字段，已保留总用量",
                );
            }
        };
    }
    if target != Protocol::AnthropicMessages {
        drop_count!(
            cache.write_short_input_tokens,
            "usage.cache.write_short_input_tokens"
        );
        drop_count!(
            cache.write_long_input_tokens,
            "usage.cache.write_long_input_tokens"
        );
    }
    if target == Protocol::Gemini {
        drop_count!(cache.write_input_tokens, "usage.cache.write_input_tokens");
    }
    let input = &mut usage.input_details;
    let output = &mut usage.output_details;
    if !matches!(target, Protocol::OpenAiChat | Protocol::Gemini) {
        drop_count!(input.text_tokens, "usage.input_details.text_tokens");
        drop_count!(input.audio_tokens, "usage.input_details.audio_tokens");
        drop_count!(input.image_tokens, "usage.input_details.image_tokens");
        drop_count!(output.text_tokens, "usage.output_details.text_tokens");
        drop_count!(output.audio_tokens, "usage.output_details.audio_tokens");
    }
    if target != Protocol::Gemini {
        for (details, path) in [
            (&mut cache.read_details, "usage.cache.read_details"),
            (&mut input.tool_details, "usage.input_details.tool_details"),
        ] {
            if *details != Default::default() {
                warn(
                    warnings,
                    source,
                    target,
                    path,
                    "目标协议没有对应的模态细分字段，已保留总用量",
                );
                *details = Default::default();
            }
        }
        drop_count!(input.video_tokens, "usage.input_details.video_tokens");
        drop_count!(input.document_tokens, "usage.input_details.document_tokens");
        drop_count!(input.tool_tokens, "usage.input_details.tool_tokens");
        drop_count!(output.image_tokens, "usage.output_details.image_tokens");
        drop_count!(output.video_tokens, "usage.output_details.video_tokens");
    }
    if target != Protocol::OpenAiChat {
        drop_count!(
            output.accepted_prediction_tokens,
            "usage.output_details.accepted_prediction_tokens"
        );
        drop_count!(
            output.rejected_prediction_tokens,
            "usage.output_details.rejected_prediction_tokens"
        );
    }
    Ok(usage)
}
