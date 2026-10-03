//! 将通用生成参数写成目标协议字段；模型级能力限制仍由 Provider 判断。
//! 参考：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create
//! 参考：https://platform.claude.com/docs/en/api/messages/create
//! 参考：https://ai.google.dev/api/generate-content#GenerationConfig

use super::{ConversionWarning, unsupported, warn};
use crate::{adapter::Result, ir::request::Generation, protocol::Protocol};

/// 只写入显式提供的参数，目标不支持停止序列时返回有损警告。
pub(in crate::adapter::protocol_codec) fn encode(
    target: Protocol,
    source: Protocol,
    generation: &Generation,
    body: &mut crate::protocol::Request,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let mut generation = generation.clone();
    if generation.candidate_count == Some(0) {
        return Err(unsupported(
            "generation.candidate_count",
            "候选数必须大于零",
        ));
    }
    if generation.candidate_count.is_some_and(|n| n > 1)
        && matches!(
            target,
            Protocol::OpenAiResponses | Protocol::AnthropicMessages
        )
    {
        warn(
            warnings,
            source,
            target,
            "generation.candidate_count",
            "目标协议只支持单候选，已降为一个候选",
        );
        generation.candidate_count = Some(1);
    }
    if generation.max_output_tokens == Some(0) || generation.top_k == Some(0) {
        return Err(unsupported("generation", "输出上限与 top_k 必须大于零"));
    }
    for (path, value) in [
        ("frequency_penalty", generation.frequency_penalty),
        ("presence_penalty", generation.presence_penalty),
    ] {
        if value.is_some_and(|v| !v.is_finite() || !(-2.0..=2.0).contains(&v)) {
            return Err(unsupported(path, "惩罚参数必须在 -2 到 2 之间"));
        }
    }
    macro_rules! drop_field {
        ($field:ident) => {
            if generation.$field.take().is_some() {
                warn(
                    warnings,
                    source,
                    target,
                    concat!("generation.", stringify!($field)),
                    "目标协议没有对应生成参数，已丢弃",
                );
            }
        };
    }
    if !matches!(target, Protocol::AnthropicMessages | Protocol::Gemini) {
        drop_field!(top_k);
    }
    if !matches!(target, Protocol::OpenAiChat | Protocol::Gemini) {
        drop_field!(seed);
        drop_field!(frequency_penalty);
        drop_field!(presence_penalty);
        drop_field!(logprobs);
        drop_field!(top_logprobs);
    }
    if generation.top_logprobs.is_some_and(|n| n > 20) {
        return Err(unsupported(
            "top_logprobs",
            "目标协议最多允许 20 个概率候选",
        ));
    }
    let max_temperature = if target == Protocol::AnthropicMessages {
        1.0
    } else {
        2.0
    };
    for (path, value, max) in [
        ("temperature", generation.temperature, max_temperature),
        ("top_p", generation.top_p, 1.0),
    ] {
        if value.is_some_and(|value| !value.is_finite() || !(0.0..=max).contains(&value)) {
            return Err(unsupported(path, "超出目标协议允许范围"));
        }
    }
    if let Some(stops) = &generation.stop_sequences {
        let max = match target {
            Protocol::OpenAiChat => 4,
            Protocol::Gemini => 5,
            _ => usize::MAX,
        };
        if stops.len() > max {
            return Err(unsupported(
                "stop_sequences",
                "超过目标协议的停止序列数量上限",
            ));
        }
        if target == Protocol::OpenAiResponses {
            warn(
                warnings,
                source,
                target,
                "generation.stop_sequences",
                "Responses 无对应停止序列字段，已丢弃",
            );
        }
    }
    write_scalars(body, &generation, None)?;
    super::controls::write(body, &generation, None, source, warnings)
}

/// 直接写入协议字段；同协议回写只更新与 before 不同的参数。
fn write_scalars(
    body: &mut crate::protocol::Request,
    generation: &Generation,
    before: Option<&Generation>,
) -> Result<()> {
    use crate::protocol::{
        OptionalNullable as O, Request, chat::request::parameters::StopSequences,
    };
    macro_rules! set {
        ($body:expr, $field:ident, $ir:ident) => {
            if before.is_none_or(|old| old.$ir != generation.$ir) {
                $body.$field = generation.$ir.clone().into();
            }
        };
    }
    match body {
        Request::Chat(body) => {
            if before.is_none_or(|old| old.max_output_tokens != generation.max_output_tokens) {
                // 旧字段与新字段不能同时留下互相冲突的输出上限。
                if body.max_completion_tokens.is_missing() && !body.max_tokens.is_missing() {
                    body.max_tokens = generation.max_output_tokens.into();
                } else {
                    body.max_completion_tokens = generation.max_output_tokens.into();
                    body.max_tokens = O::Missing;
                }
            }
            set!(body, seed, seed);
            set!(body, frequency_penalty, frequency_penalty);
            set!(body, presence_penalty, presence_penalty);
            set!(body, logprobs, logprobs);
            set!(body, top_logprobs, top_logprobs);
            set!(body, temperature, temperature);
            set!(body, top_p, top_p);
            set!(body, n, candidate_count);
            if before.is_none_or(|old| old.stop_sequences != generation.stop_sequences) {
                body.stop = generation
                    .stop_sequences
                    .clone()
                    .map(StopSequences::Many)
                    .into();
            }
            if before.is_some_and(|old| old.stream != generation.stream) {
                body.stream = O::Value(generation.stream);
            }
        }
        Request::Responses(body) => {
            set!(body, max_output_tokens, max_output_tokens);
            set!(body, temperature, temperature);
            set!(body, top_p, top_p);
            if before.is_some_and(|old| old.stream != generation.stream) {
                body.stream = O::Value(generation.stream);
            }
        }
        Request::Messages(body) => {
            set!(body, top_k, top_k);
            if before.is_none_or(|old| old.max_output_tokens != generation.max_output_tokens) {
                body.max_tokens = generation
                    .max_output_tokens
                    .ok_or_else(|| unsupported("max_tokens", "Messages 请求必须明确输出上限"))?;
            }
            set!(body, temperature, temperature);
            set!(body, top_p, top_p);
            set!(body, stop_sequences, stop_sequences);
            if before.is_some_and(|old| old.stream != generation.stream) {
                body.stream = O::Value(generation.stream);
            }
        }
        Request::Gemini(body) => {
            if generation.stream {
                return Err(unsupported("stream", "Gemini 流式模式位于 URL"));
            }
            let mut config = body
                .generation_config
                .as_option()
                .cloned()
                .unwrap_or_default();
            set!(config, max_output_tokens, max_output_tokens);
            set!(config, top_k, top_k);
            set!(config, seed, seed);
            set!(config, frequency_penalty, frequency_penalty);
            set!(config, presence_penalty, presence_penalty);
            set!(config, response_logprobs, logprobs);
            set!(config, logprobs, top_logprobs);
            set!(config, temperature, temperature);
            set!(config, top_p, top_p);
            set!(config, stop_sequences, stop_sequences);
            set!(config, candidate_count, candidate_count);
            if config != Default::default() {
                body.generation_config = O::Value(config);
            } else if !body.generation_config.is_missing() {
                body.generation_config = O::Missing;
            }
        }
    }
    Ok(())
}

/// 同协议只回写实际编辑的参数，不能无提示地丢弃约束。
pub(in crate::adapter::protocol_codec) fn write(
    body: &mut crate::protocol::Request,
    generation: &Generation,
    before: Option<&Generation>,
) -> Result<()> {
    write_scalars(body, generation, before)?;
    let mut warnings = vec![];
    super::controls::write(body, generation, before, body.protocol(), &mut warnings)?;
    if !warnings.is_empty() {
        return Err(unsupported(
            "generation",
            "编辑后的参数无法在同协议无损表达",
        ));
    }
    Ok(())
}
