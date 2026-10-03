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
    if generation.candidate_count.is_some_and(|count| count != 1) {
        return Err(unsupported(
            "generation.candidate_count",
            "当前转换仅支持单候选",
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
    write(body, generation, None)
}

/// 直接写入协议字段；同协议回写只更新与 before 不同的参数。
pub(in crate::adapter::protocol_codec) fn write(
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
            set!(body, max_completion_tokens, max_output_tokens);
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
