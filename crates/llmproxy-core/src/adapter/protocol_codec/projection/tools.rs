//! 客户端函数声明的类型化投影。
use super::Notes;
use crate::{
    ir::request::Function,
    protocol::{
        OptionalNullable as O, Request, chat::request::parameters::Tool as ChatTool,
        messages::request::tool::Tool as MessagesTool, responses::function::Tool as ResponsesTool,
    },
};
use serde_json::Value;

/// 从各协议函数类型读取已知字段，未知工具不进入通用函数列表。
pub(super) fn decode(source: &Request, notes: &mut Notes) -> Vec<Function> {
    let mut result = Vec::new();
    match source {
        Request::Chat(body) => {
            for (i, tool) in body.tools.as_option().into_iter().flatten().enumerate() {
                let path = format!("tools[{i}]");
                if let ChatTool::Function { function, extra } = tool {
                    notes.extra(extra, &path);
                    notes.extra(&function.extra, &path);
                    add(
                        &mut result,
                        notes,
                        &path,
                        &function.name,
                        &function.description,
                        &function.parameters,
                        &function.strict,
                    );
                } else {
                    notes.dropped(path);
                }
            }
        }
        Request::Responses(body) => {
            for (i, tool) in body.tools.as_option().into_iter().flatten().enumerate() {
                let path = format!("tools[{i}]");
                if let ResponsesTool::Function(function) = tool {
                    notes.extra(&function.extra, &path);
                    add(
                        &mut result,
                        notes,
                        &path,
                        &function.name,
                        &function.description,
                        &function.parameters,
                        &function.strict,
                    );
                } else {
                    notes.dropped(path);
                }
            }
        }
        Request::Messages(body) => {
            for (i, tool) in body.tools.as_option().into_iter().flatten().enumerate() {
                let path = format!("tools[{i}]");
                if let MessagesTool::Function(function) = tool {
                    if function
                        .r#type
                        .as_option()
                        .is_some_and(|kind| kind != "custom")
                    {
                        notes.dropped(path);
                        continue;
                    }
                    notes.extra(&function.extra, &path);
                    add(
                        &mut result,
                        notes,
                        &path,
                        &function.name,
                        &function.description,
                        &O::Value(function.input_schema.clone()),
                        &O::Missing,
                    );
                } else {
                    notes.dropped(path);
                }
            }
        }
        Request::Gemini(body) => {
            for (i, tool) in body.tools.as_option().into_iter().flatten().enumerate() {
                let path = format!("tools[{i}]");
                notes.field(
                    &tool.google_search_retrieval,
                    &format!("{path}.googleSearchRetrieval"),
                );
                notes.field(&tool.code_execution, &format!("{path}.codeExecution"));
                notes.field(&tool.google_search, &format!("{path}.googleSearch"));
                notes.field(&tool.computer_use, &format!("{path}.computerUse"));
                notes.field(&tool.url_context, &format!("{path}.urlContext"));
                notes.field(&tool.file_search, &format!("{path}.fileSearch"));
                notes.field(&tool.mcp_servers, &format!("{path}.mcpServers"));
                notes.field(&tool.google_maps, &format!("{path}.googleMaps"));
                notes.extra(&tool.extra, &path);
                for (j, function) in tool
                    .function_declarations
                    .as_option()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let path = format!("{path}.functionDeclarations[{j}]");
                    notes.extra(&function.extra, &path);
                    notes.field(&function.behavior, &format!("{path}.behavior"));
                    notes.field(&function.response, &format!("{path}.response"));
                    notes.field(
                        &function.response_json_schema,
                        &format!("{path}.responseJsonSchema"),
                    );
                    let schema = if !function.parameters_json_schema.is_missing() {
                        function.parameters_json_schema.clone()
                    } else {
                        function
                            .parameters
                            .as_option()
                            .map(|v| json_schema(Value::Object(v.clone())))
                            .into()
                    };
                    add(
                        &mut result,
                        notes,
                        &path,
                        &function.name,
                        &function.description,
                        &schema,
                        &O::Missing,
                    );
                }
            }
        }
    }
    result
}
/// 函数参数是动态 JSON Schema；不允许缺失或非对象的 Schema 进入转换。
fn add(
    result: &mut Vec<Function>,
    notes: &mut Notes,
    path: &str,
    name: &str,
    description: &O<String>,
    schema: &O<Value>,
    strict: &O<bool>,
) {
    let Some(parameters) = schema.as_option().filter(|value| value.is_object()) else {
        notes.reject(path, "函数声明缺少有效的参数 Schema");
        return;
    };
    result.push(Function {
        name: name.into(),
        description: description.as_option().cloned(),
        parameters: parameters.clone(),
        strict: strict.as_option().copied(),
    });
}
/// Gemini 原生 Schema 的类型枚举使用大写；仅规范化 Schema 节点。
fn json_schema(mut value: Value) -> Value {
    fn normalize(value: &mut Value) {
        if let Value::Object(object) = value {
            if let Some(Value::String(kind)) = object.get_mut("type") {
                *kind = kind.to_ascii_lowercase();
            }
            if let Some(Value::Object(properties)) = object.get_mut("properties") {
                for child in properties.values_mut() {
                    normalize(child);
                }
            }
            if let Some(child) = object.get_mut("items") {
                normalize(child);
            }
            if let Some(Value::Array(children)) = object.get_mut("anyOf") {
                for child in children {
                    normalize(child);
                }
            }
        }
    }
    normalize(&mut value);
    value
}
