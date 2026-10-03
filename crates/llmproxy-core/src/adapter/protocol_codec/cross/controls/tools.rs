use super::super::{ConversionWarning, unsupported, warn};
use crate::{
    adapter::Result,
    ir::request::{Generation, controls::ToolChoice},
    protocol::{
        OptionalNullable as O, Protocol, Request, chat::request::parameters as c,
        gemini::request::body as g, messages::request::body as m,
    },
};
use serde_json::{Map, Value};
/// 指定工具与允许列表必须在目标函数声明中存在，禁止扩大工具集合。
pub(super) fn encode(
    body: &mut Request,
    generation: &Generation,
    source: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = body.protocol();
    let names = match &generation.tool_choice {
        Some(ToolChoice::Named(name)) => vec![name.clone()],
        Some(ToolChoice::Allowed { names, .. }) => names.clone(),
        _ => vec![],
    };
    let declared = match body {
        Request::Chat(b) => b
            .tools
            .as_option()
            .into_iter()
            .flatten()
            .filter_map(|t| match t {
                c::Tool::Function { function, .. } => Some(function.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        Request::Responses(b) => b
            .tools
            .as_option()
            .into_iter()
            .flatten()
            .filter_map(|t| match t {
                crate::protocol::responses::function::Tool::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect(),
        Request::Messages(b) => b
            .tools
            .as_option()
            .into_iter()
            .flatten()
            .filter_map(|t| match t {
                crate::protocol::messages::request::tool::Tool::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect(),
        Request::Gemini(b) => b
            .tools
            .as_option()
            .into_iter()
            .flatten()
            .flat_map(|t| {
                t.function_declarations
                    .as_option()
                    .into_iter()
                    .flatten()
                    .map(|f| f.name.clone())
            })
            .collect(),
    };
    let has_native = match &body {
        Request::Responses(b) => b.tools.as_option().is_some_and(|tools| {
            tools
                .iter()
                .any(|t| matches!(t, crate::protocol::responses::function::Tool::Other(_)))
        }),
        Request::Messages(b) => b.tools.as_option().is_some_and(|tools| {
            tools
                .iter()
                .any(|t| matches!(t, crate::protocol::messages::request::tool::Tool::Other(_)))
        }),
        _ => false,
    };
    if names.iter().any(|n| !declared.contains(n))
        || matches!(generation.tool_choice, Some(ToolChoice::Required))
            && declared.is_empty()
            && !has_native
    {
        return Err(unsupported("tool_choice", "指定工具不存在或没有可调用工具"));
    }
    if let Some(ToolChoice::Allowed { names, .. }) = &generation.tool_choice
        && names.is_empty()
    {
        return Err(unsupported("tool_choice", "允许列表不能为空"));
    }
    match body {
        Request::Chat(b) => {
            b.parallel_tool_calls = generation.parallel_tool_calls.into();
            b.tool_choice = generation
                .tool_choice
                .as_ref()
                .map(|v| match v {
                    ToolChoice::Named(name) => {
                        c::ToolChoice::Specific(c::SpecificToolChoice::Function {
                            function: c::NamedTool {
                                name: name.clone(),
                                extra: Default::default(),
                            },
                            extra: Default::default(),
                        })
                    }
                    ToolChoice::Allowed { required, names } => {
                        c::ToolChoice::Specific(c::SpecificToolChoice::AllowedTools {
                            allowed_tools: c::AllowedTools {
                                mode: if *required { "required" } else { "auto" }.into(),
                                tools: names
                                    .iter()
                                    .map(|name| {
                                        Map::from_iter([
                                            ("type".into(), Value::String("function".into())),
                                            (
                                                "function".into(),
                                                Value::Object(Map::from_iter([(
                                                    "name".into(),
                                                    Value::String(name.clone()),
                                                )])),
                                            ),
                                        ])
                                    })
                                    .collect(),
                                extra: Default::default(),
                            },
                            extra: Default::default(),
                        })
                    }
                    v => c::ToolChoice::Mode(mode(v).into()),
                })
                .into();
        }
        Request::Responses(b) => {
            b.parallel_tool_calls = generation.parallel_tool_calls.into();
            b.tool_choice = generation.tool_choice.as_ref().map(|v| match v {
                ToolChoice::Named(name) => serde_json::json!({"type":"function","name":name}),
                ToolChoice::Allowed {required,names} => serde_json::json!({"type":"allowed_tools","mode":if *required {"required"} else {"auto"},"tools":names.iter().map(|n| serde_json::json!({"type":"function","name":n})).collect::<Vec<_>>()}),
                v => Value::String(mode(v).into()),
            }).into();
        }
        Request::Messages(b) => {
            if let Some(ToolChoice::Allowed { names, .. }) = &generation.tool_choice {
                if let O::Value(tools) = &mut b.tools {
                    tools.retain(|t| matches!(t, crate::protocol::messages::request::tool::Tool::Function(f) if names.contains(&f.name)));
                }
                warn(
                    warnings,
                    source,
                    target,
                    "tool_choice.allowed",
                    "目标无允许列表，通过收窄工具声明保持调用约束",
                );
            }
            b.tool_choice = if generation.tool_choice.is_none()
                && generation.parallel_tool_calls.is_none()
            {
                O::Missing
            } else {
                O::Value(m::ToolChoice {
                    r#type: match &generation.tool_choice {
                        Some(ToolChoice::Named(_)) => "tool",
                        Some(ToolChoice::Required | ToolChoice::Allowed { required: true, .. }) => {
                            "any"
                        }
                        Some(ToolChoice::None) => "none",
                        _ => "auto",
                    }
                    .into(),
                    name: if let Some(ToolChoice::Named(name)) = &generation.tool_choice {
                        O::Value(name.clone())
                    } else {
                        O::Missing
                    },
                    disable_parallel_tool_use: generation.parallel_tool_calls.map(|v| !v).into(),
                    extra: Default::default(),
                })
            };
        }
        Request::Gemini(b) => {
            b.tool_config = generation
                .tool_choice
                .as_ref()
                .map(|v| g::ToolConfig {
                    function_calling_config: O::Value(g::FunctionCallingConfig {
                        mode: O::Value(
                            match v {
                                ToolChoice::None => "NONE",
                                ToolChoice::Named(_)
                                | ToolChoice::Required
                                | ToolChoice::Allowed { required: true, .. } => "ANY",
                                _ => "AUTO",
                            }
                            .into(),
                        ),
                        allowed_function_names: if names.is_empty() {
                            O::Missing
                        } else {
                            O::Value(names)
                        },
                        extra: Default::default(),
                    }),
                    extra: Default::default(),
                })
                .into();
            if generation.parallel_tool_calls.is_some() {
                warn(
                    warnings,
                    source,
                    target,
                    "parallel_tool_calls",
                    "Gemini 无等价的并行调用开关，已丢弃",
                );
            }
        }
    }
    Ok(())
}

/// 普通模式名称仅用于已经排除具名／列表变体的分支。
fn mode(choice: &ToolChoice) -> &'static str {
    match choice {
        ToolChoice::None => "none",
        ToolChoice::Required => "required",
        _ => "auto",
    }
}
