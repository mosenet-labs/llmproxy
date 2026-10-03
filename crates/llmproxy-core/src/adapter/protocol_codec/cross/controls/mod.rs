//! 工具选择、输出格式和推理设置的目标字段构造。
//! 参考：https://developers.openai.com/api/reference/resources/responses/methods/create
//! 参考：https://ai.google.dev/api/generate-content#GenerationConfig
use super::{ConversionWarning, unsupported, warn};
use crate::{
    adapter::Result,
    ir::request::Generation,
    protocol::{
        OptionalNullable as O, Protocol, Request, gemini::request::body as g,
        messages::request::body as m, responses::request::body as r,
    },
};

/// 只更新改变的约束；同协议校验复用这一入口，避免忽略 IR 编辑。
pub(in crate::adapter::protocol_codec::cross) fn write(
    body: &mut Request,
    generation: &Generation,
    before: Option<&Generation>,
    source: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = body.protocol();
    if before.is_none_or(|old| {
        old.tool_choice != generation.tool_choice
            || old.parallel_tool_calls != generation.parallel_tool_calls
    }) {
        tools::encode(body, generation, source, warnings)?;
    }
    if before.is_none_or(|old| old.output_format != generation.output_format) {
        output::encode(body, generation.output_format.as_ref(), source, warnings)?;
    }
    if before.is_none_or(|old| old.reasoning != generation.reasoning) {
        let reasoning = &generation.reasoning;
        if reasoning.budget.is_some_and(|n| n < -1) {
            return Err(unsupported("reasoning.budget", "预算不能小于 -1"));
        }
        if reasoning
            .mode
            .as_deref()
            .is_some_and(|m| !matches!(m, "enabled" | "disabled" | "adaptive"))
        {
            return Err(unsupported("reasoning.mode", "未知推理模式"));
        }
        match body {
            Request::Chat(body) => {
                body.reasoning_effort = reasoning
                    .effort
                    .clone()
                    .or_else(|| {
                        (reasoning.mode.as_deref() == Some("disabled")
                            || reasoning.budget == Some(0))
                        .then(|| "none".into())
                    })
                    .into();
                if reasoning.budget.is_some_and(|n| n != 0)
                    || reasoning.mode.as_deref().is_some_and(|m| m != "disabled")
                {
                    warn(
                        warnings,
                        source,
                        target,
                        "reasoning.budget",
                        "目标只有努力等级，无法换算词元预算／自动模式，已保留可用的努力等级",
                    );
                }
                if reasoning.summary.is_some() || reasoning.include.is_some() {
                    warn(
                        warnings,
                        source,
                        target,
                        "reasoning.summary",
                        "Chat 无思考摘要开关，已丢弃",
                    );
                }
            }
            Request::Responses(body) => {
                body.reasoning = if reasoning == &Default::default() {
                    O::Missing
                } else {
                    O::Value(r::Reasoning {
                        context: O::Missing,
                        effort: reasoning
                            .effort
                            .clone()
                            .or_else(|| {
                                (reasoning.mode.as_deref() == Some("disabled")
                                    || reasoning.budget == Some(0))
                                .then(|| "none".into())
                            })
                            .into(),
                        generate_summary: O::Missing,
                        summary: reasoning
                            .summary
                            .clone()
                            .or_else(|| (reasoning.include == Some(true)).then(|| "auto".into()))
                            .into(),
                        extra: Default::default(),
                    })
                };
                if reasoning.budget.is_some_and(|n| n != 0)
                    || reasoning.mode.as_deref().is_some_and(|m| m != "disabled")
                {
                    warn(
                        warnings,
                        source,
                        target,
                        "reasoning.budget",
                        "目标只有努力等级，无法换算词元预算／自动模式",
                    );
                }
            }
            Request::Messages(body) => {
                let mode = reasoning.mode.clone().or_else(|| match reasoning.budget {
                    Some(0) => Some("disabled".into()),
                    Some(n) if n > 0 => Some("enabled".into()),
                    Some(-1) => Some("adaptive".into()),
                    _ => reasoning.effort.as_ref().map(|e| {
                        if e == "none" {
                            "disabled".into()
                        } else {
                            "adaptive".into()
                        }
                    }),
                });
                if mode.as_deref() == Some("enabled") && reasoning.budget.is_none_or(|n| n < 1024) {
                    return Err(unsupported(
                        "reasoning.budget",
                        "Messages enabled 思考预算至少为 1024",
                    ));
                }
                if mode.as_deref() == Some("enabled")
                    && reasoning
                        .budget
                        .zip(generation.max_output_tokens)
                        .is_some_and(|(budget, max)| budget as u64 >= max)
                {
                    return Err(unsupported("reasoning.budget", "思考预算必须小于输出上限"));
                }
                body.thinking = mode
                    .map(|mode| m::Thinking {
                        budget_tokens: reasoning
                            .budget
                            .filter(|n| *n > 0 && mode == "enabled")
                            .map(|n| n as u64)
                            .into(),
                        r#type: mode,
                        extra: Default::default(),
                    })
                    .into();
                if reasoning.effort.is_some() || body.output_config.as_option().is_some() {
                    let config = ensure_output_config(&mut body.output_config);
                    config.effort = reasoning.effort.clone().filter(|e| e != "none").into();
                }
                if reasoning.summary.is_some() || reasoning.include.is_some() {
                    warn(
                        warnings,
                        source,
                        target,
                        "reasoning.summary",
                        "Messages 的思考输出由模型控制，无法保持摘要开关",
                    );
                }
            }
            Request::Gemini(body) => {
                if reasoning != &Default::default() || body.generation_config.as_option().is_some()
                {
                    let config = ensure_gemini(&mut body.generation_config);
                    config.thinking_config = if reasoning == &Default::default() {
                        O::Missing
                    } else {
                        O::Value(g::ThinkingConfig {
                            include_thoughts: reasoning
                                .include
                                .or(reasoning.summary.as_ref().map(|_| true))
                                .into(),
                            thinking_budget: reasoning
                                .budget
                                .or(match reasoning.mode.as_deref() {
                                    Some("disabled") => Some(0),
                                    Some("adaptive") => Some(-1),
                                    _ if reasoning.effort.as_deref() == Some("none") => Some(0),
                                    _ => None,
                                })
                                .into(),
                            thinking_level: reasoning.effort.clone().filter(|e| e != "none").into(),
                            extra: Default::default(),
                        })
                    };
                }
                if reasoning.summary.is_some() {
                    warn(
                        warnings,
                        source,
                        target,
                        "reasoning.summary",
                        "仅映射为包含思考，摘要详细程度无法保持",
                    );
                }
            }
        }
    }
    Ok(())
}

mod output;
mod tools;
use output::{ensure_gemini, ensure_output_config};
