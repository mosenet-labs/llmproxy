//! 将通用函数声明直接写入目标协议类型。
use super::{ConversionWarning, unsupported, warn};
use crate::{
    adapter::Result,
    ir::request::Function,
    protocol::{OptionalNullable as O, Protocol, Request, chat, gemini, messages, responses},
};

/// 已知函数字段不经过 JSON 对象拼装；Schema 本身保持动态类型。
pub(in crate::adapter::protocol_codec) fn encode_tools(
    source: Protocol,
    body: &mut Request,
    tools: &[Function],
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = body.protocol();
    for (i, tool) in tools.iter().enumerate() {
        if tool.name.is_empty() || !tool.parameters.is_object() {
            return Err(unsupported(
                &format!("tools[{i}]"),
                "函数名称不能为空且参数 Schema 必须是对象",
            ));
        }
        if tool.strict.is_some() && matches!(target, Protocol::AnthropicMessages | Protocol::Gemini)
        {
            warn(
                warnings,
                source,
                target,
                &format!("tools[{i}].strict"),
                "严格 Schema 模式无对应字段，已丢弃",
            );
        }
    }
    match body {
        Request::Chat(body) => {
            body.tools = nonempty(
                tools
                    .iter()
                    .map(|f| chat::request::parameters::Tool::Function {
                        function: chat::request::parameters::FunctionDefinition {
                            name: f.name.clone(),
                            description: f.description.clone().into(),
                            parameters: O::Value(f.parameters.clone()),
                            strict: f.strict.into(),
                            extra: Default::default(),
                        },
                        extra: Default::default(),
                    })
                    .collect(),
            )
        }
        Request::Responses(body) => {
            body.tools = nonempty(
                tools
                    .iter()
                    .map(|f| {
                        responses::function::Tool::Function(responses::function::Function {
                            r#type: responses::function::FunctionType::Function,
                            name: f.name.clone(),
                            description: f.description.clone().into(),
                            parameters: O::Value(f.parameters.clone()),
                            strict: f.strict.into(),
                            extra: Default::default(),
                        })
                    })
                    .collect(),
            )
        }
        Request::Messages(body) => {
            body.tools = nonempty(
                tools
                    .iter()
                    .map(|f| {
                        messages::request::tool::Tool::Function(messages::request::tool::Function {
                            name: f.name.clone(),
                            description: f.description.clone().into(),
                            input_schema: f.parameters.clone(),
                            r#type: O::Missing,
                            extra: Default::default(),
                        })
                    })
                    .collect(),
            )
        }
        Request::Gemini(body) => {
            body.tools = if tools.is_empty() {
                O::Missing
            } else {
                O::Value(vec![gemini::request::body::Tool {
                    function_declarations: O::Value(
                        tools
                            .iter()
                            .map(|f| gemini::request::body::FunctionDeclaration {
                                name: f.name.clone(),
                                description: f.description.clone().into(),
                                parameters_json_schema: O::Value(f.parameters.clone()),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                    ..Default::default()
                }])
            }
        }
    }
    Ok(())
}
/// 空工具列表不生成字段。
fn nonempty<T>(items: Vec<T>) -> O<Vec<T>> {
    if items.is_empty() {
        O::Missing
    } else {
        O::Value(items)
    }
}
