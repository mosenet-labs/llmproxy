//! 类型化请求约束投影；开放的局部工具选择对象在此解释，不中转整个报文。
use super::Notes;
use crate::{
    ir::request::{
        Generation,
        controls::{OutputFormat, ToolChoice},
    },
    protocol::{Request, chat::request::parameters as c},
};

/// 抽取四协议工具、格式及推理配置，未知模式显式阻止有损放宽约束。
pub(super) fn decode(source: &Request, generation: &mut Generation, notes: &mut Notes) {
    match source {
        Request::Chat(body) => {
            generation.parallel_tool_calls = body.parallel_tool_calls.as_option().copied();
            generation.tool_choice = body
                .tool_choice
                .as_option()
                .and_then(|choice| match choice {
                    c::ToolChoice::Mode(value) => mode(value, notes),
                    c::ToolChoice::Specific(c::SpecificToolChoice::Function {
                        function,
                        extra,
                    }) => {
                        notes.extra(extra, "tool_choice");
                        notes.extra(&function.extra, "tool_choice.function");
                        Some(ToolChoice::Named(function.name.clone()))
                    }
                    c::ToolChoice::Specific(c::SpecificToolChoice::AllowedTools {
                        allowed_tools,
                        extra,
                    }) => {
                        notes.extra(extra, "tool_choice");
                        notes.extra(&allowed_tools.extra, "tool_choice.allowed_tools");
                        for (index, tool) in allowed_tools.tools.iter().enumerate() {
                            let path = format!("tool_choice.allowed_tools.tools[{index}]");
                            notes.object_extra(tool, &["type", "function"], &path);
                            if let Some(function) =
                                tool.get("function").and_then(serde_json::Value::as_object)
                            {
                                notes.object_extra(
                                    function,
                                    &["name"],
                                    &format!("{path}.function"),
                                );
                            }
                        }
                        allowed(
                            &allowed_tools.mode,
                            allowed_tools.tools.iter().map(|tool| {
                                (
                                    tool.get("type").and_then(serde_json::Value::as_str),
                                    tool.get("function")
                                        .and_then(|f| f.get("name"))
                                        .and_then(serde_json::Value::as_str),
                                )
                            }),
                            notes,
                        )
                    }
                    _ => {
                        notes.reject("tool_choice", "此工具选择类型尚无等价映射");
                        None
                    }
                });
            generation.output_format =
                body.response_format
                    .as_option()
                    .and_then(|format| match format {
                        c::ResponseFormat::Text { extra } => {
                            notes.extra(extra, "response_format");
                            Some(OutputFormat::Text)
                        }
                        c::ResponseFormat::JsonObject { extra } => {
                            notes.extra(extra, "response_format");
                            Some(OutputFormat::JsonObject)
                        }
                        c::ResponseFormat::JsonSchema {
                            json_schema: schema,
                            extra,
                        } => {
                            notes.extra(extra, "response_format");
                            notes.extra(&schema.extra, "response_format.json_schema");
                            schema
                                .schema
                                .as_option()
                                .map(|value| OutputFormat::JsonSchema {
                                    name: Some(schema.name.clone()),
                                    description: schema.description.as_option().cloned(),
                                    schema: value.clone(),
                                    strict: schema.strict.as_option().copied(),
                                })
                                .or_else(|| {
                                    notes.reject("response_format", "缺少 JSON Schema");
                                    None
                                })
                        }
                    });
            generation.reasoning.effort = body.reasoning_effort.as_option().cloned();
        }
        Request::Responses(body) => {
            generation.parallel_tool_calls = body.parallel_tool_calls.as_option().copied();
            if let Some(choice) = body.tool_choice.as_option() {
                generation.tool_choice = if let Some(value) = choice.as_str() {
                    mode(value, notes)
                } else if choice.get("type").and_then(serde_json::Value::as_str) == Some("function")
                {
                    if let Some(object) = choice.as_object() {
                        notes.object_extra(object, &["type", "name"], "tool_choice");
                    }
                    choice
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(|n| ToolChoice::Named(n.into()))
                        .or_else(|| {
                            notes.reject("tool_choice.name", "指定函数缺少名称");
                            None
                        })
                } else if choice.get("type").and_then(serde_json::Value::as_str)
                    == Some("allowed_tools")
                {
                    if let Some(object) = choice.as_object() {
                        notes.object_extra(object, &["type", "mode", "tools"], "tool_choice");
                    }
                    match (
                        choice.get("mode").and_then(serde_json::Value::as_str),
                        choice.get("tools").and_then(serde_json::Value::as_array),
                    ) {
                        (Some(mode), Some(tools)) => {
                            for (index, tool) in tools.iter().enumerate() {
                                if let Some(object) = tool.as_object() {
                                    notes.object_extra(
                                        object,
                                        &["type", "name"],
                                        &format!("tool_choice.tools[{index}]"),
                                    );
                                }
                            }
                            allowed(
                                mode,
                                tools.iter().map(|tool| {
                                    (
                                        tool.get("type").and_then(serde_json::Value::as_str),
                                        tool.get("name").and_then(serde_json::Value::as_str),
                                    )
                                }),
                                notes,
                            )
                        }
                        _ => {
                            notes.reject("tool_choice", "允许列表缺少模式或工具");
                            None
                        }
                    }
                } else {
                    notes.reject("tool_choice", "此工具选择类型尚无等价映射");
                    None
                };
            }
            if let Some(config) = body.text.as_option() {
                notes.extra(&config.extra, "text");
                notes.field(&config.verbosity, "text.verbosity");
                if let Some(format) = config.format.as_option() {
                    notes.extra(&format.extra, "text.format");
                    generation.output_format = match format.r#type.as_str() {
                        "text" => Some(OutputFormat::Text),
                        "json_object" => Some(OutputFormat::JsonObject),
                        "json_schema" => format
                            .schema
                            .as_option()
                            .map(|schema| OutputFormat::JsonSchema {
                                name: format.name.as_option().cloned(),
                                description: format.description.as_option().cloned(),
                                schema: schema.clone(),
                                strict: format.strict.as_option().copied(),
                            })
                            .or_else(|| {
                                notes.reject("text.format", "缺少 JSON Schema");
                                None
                            }),
                        _ => {
                            notes.reject("text.format", "未知输出格式");
                            None
                        }
                    };
                }
            }
            if let Some(reasoning) = body.reasoning.as_option() {
                generation.reasoning.effort = reasoning.effort.as_option().cloned();
                generation.reasoning.summary = reasoning
                    .summary
                    .as_option()
                    .or(reasoning.generate_summary.as_option())
                    .cloned();
                if reasoning.context.as_option().is_some() {
                    notes.reject(
                        "reasoning.context",
                        "推理上下文属于来源 Provider，无法跨协议复用",
                    );
                }
                notes.extra(&reasoning.extra, "reasoning");
            }
        }
        Request::Messages(body) => {
            if let Some(choice) = body.tool_choice.as_option() {
                notes.extra(&choice.extra, "tool_choice");
                generation.parallel_tool_calls =
                    choice.disable_parallel_tool_use.as_option().map(|v| !v);
                generation.tool_choice = match choice.r#type.as_str() {
                    "tool" => choice
                        .name
                        .as_option()
                        .map(|name| ToolChoice::Named(name.clone()))
                        .or_else(|| {
                            notes.reject("tool_choice.name", "指定工具缺少名称");
                            None
                        }),
                    value => mode(value, notes),
                };
            }
            if let Some(output) = body.output_config.as_option() {
                generation.reasoning.effort = output.effort.as_option().cloned();
                notes.extra(&output.extra, "output_config");
                if let Some(format) = output.format.as_option() {
                    notes.extra(&format.extra, "output_config.format");
                    if format.r#type == "json_schema" {
                        generation.output_format = Some(OutputFormat::JsonSchema {
                            name: None,
                            description: None,
                            schema: format.schema.clone(),
                            strict: None,
                        });
                    } else {
                        notes.reject("output_config.format", "未知输出格式");
                    }
                }
            }
            if let Some(thinking) = body.thinking.as_option() {
                generation.reasoning.mode = Some(thinking.r#type.clone());
                generation.reasoning.budget =
                    thinking
                        .budget_tokens
                        .as_option()
                        .and_then(|n| match i64::try_from(*n) {
                            Ok(n) => Some(n),
                            Err(_) => {
                                notes.reject("thinking.budget_tokens", "推理预算超出可表达范围");
                                None
                            }
                        });
                notes.extra(&thinking.extra, "thinking");
            }
        }
        Request::Gemini(body) => {
            if let Some(config) = body.tool_config.as_option() {
                notes.extra(&config.extra, "toolConfig");
                if let Some(choice) = config.function_calling_config.as_option() {
                    notes.extra(&choice.extra, "toolConfig.functionCallingConfig");
                    let choice_mode = choice
                        .mode
                        .as_option()
                        .map(String::as_str)
                        .unwrap_or("AUTO");
                    let required = choice_mode == "ANY";
                    generation.tool_choice = if let Some(names) = choice
                        .allowed_function_names
                        .as_option()
                        .filter(|v| !v.is_empty())
                    {
                        if matches!(choice_mode, "AUTO" | "ANY") {
                            Some(ToolChoice::Allowed {
                                required,
                                names: names.clone(),
                            })
                        } else {
                            notes.reject(
                                "toolConfig.functionCallingConfig",
                                "此模式无法与函数允许列表等价映射",
                            );
                            None
                        }
                    } else {
                        choice.mode.as_option().and_then(|m| mode(m, notes))
                    };
                }
            }
            if let Some(config) = body.generation_config.as_option() {
                let schema = config
                    .response_json_schema
                    .as_option()
                    .cloned()
                    .or_else(|| {
                        config.response_schema.as_option().map(|s| {
                            super::tools::json_schema(serde_json::Value::Object(s.clone()))
                        })
                    });
                generation.output_format = if let Some(schema) = schema {
                    schema
                        .as_object()
                        .map(|schema| OutputFormat::JsonSchema {
                            name: None,
                            description: None,
                            schema: schema.clone(),
                            strict: None,
                        })
                        .or_else(|| {
                            notes
                                .reject("generationConfig.responseJsonSchema", "Schema 必须是对象");
                            None
                        })
                } else {
                    match config.response_mime_type.as_option().map(String::as_str) {
                        Some("application/json") => Some(OutputFormat::JsonObject),
                        Some("text/plain") => Some(OutputFormat::Text),
                        Some(_) => {
                            notes.reject(
                                "generationConfig.responseMimeType",
                                "目标不支持此输出 MIME 类型",
                            );
                            None
                        }
                        None => None,
                    }
                };
                if let Some(thinking) = config.thinking_config.as_option() {
                    generation.reasoning.effort = thinking.thinking_level.as_option().cloned();
                    generation.reasoning.budget = thinking.thinking_budget.as_option().copied();
                    generation.reasoning.include = thinking.include_thoughts.as_option().copied();
                    notes.extra(&thinking.extra, "generationConfig.thinkingConfig");
                }
            }
        }
    }
}

/// 同义模式合并为一个语义；未知模式不能当作 auto。
fn mode(value: &str, notes: &mut Notes) -> Option<ToolChoice> {
    match value {
        "auto" | "AUTO" => Some(ToolChoice::Auto),
        "none" | "NONE" => Some(ToolChoice::None),
        "required" | "any" | "ANY" => Some(ToolChoice::Required),
        _ => {
            notes.reject("tool_choice", "未知工具选择模式");
            None
        }
    }
}

/// 只接受函数允许列表；缺失名称或未知模式不能降为不限工具的调用。
fn allowed<'a>(
    mode: &str,
    tools: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
    notes: &mut Notes,
) -> Option<ToolChoice> {
    if !matches!(mode, "auto" | "required") {
        notes.reject("tool_choice", "未知允许列表模式");
        return None;
    }
    let mut names = Vec::new();
    for (kind, name) in tools {
        match (kind, name) {
            (Some("function"), Some(name)) if !name.is_empty() => names.push(name.to_owned()),
            _ => {
                notes.reject("tool_choice", "允许列表仅支持具名函数");
                return None;
            }
        }
    }
    if names.is_empty() {
        notes.reject("tool_choice", "允许列表不能为空");
        return None;
    }
    Some(ToolChoice::Allowed {
        required: mode == "required",
        names,
    })
}
