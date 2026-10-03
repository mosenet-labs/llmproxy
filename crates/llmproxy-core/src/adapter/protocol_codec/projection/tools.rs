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
                        continue;
                    }
                    let mut extra = function.extra.clone();
                    extra.remove("cache_control");
                    notes.extra(&extra, &path);
                    add(
                        &mut result,
                        notes,
                        &path,
                        &function.name,
                        &function.description,
                        &O::Value(function.input_schema.clone()),
                        &O::Missing,
                    );
                }
            }
        }
        Request::Gemini(body) => {
            for (i, tool) in body.tools.as_option().into_iter().flatten().enumerate() {
                let path = format!("tools[{i}]");
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
/// 缺省参数表示无参函数；显式 null 或非对象 Schema 不能转换。
fn add(
    result: &mut Vec<Function>,
    notes: &mut Notes,
    path: &str,
    name: &str,
    description: &O<String>,
    schema: &O<Value>,
    strict: &O<bool>,
) {
    let parameters = match schema {
        O::Missing => serde_json::json!({"type":"object","properties":{}}),
        O::Value(value) if value.is_object() => value.clone(),
        _ => {
            notes.reject(path, "函数声明缺少有效的参数 Schema");
            return;
        }
    };
    result.push(Function {
        name: name.into(),
        description: description.as_option().cloned(),
        parameters,
        strict: strict.as_option().copied(),
    });
}
/// Gemini 原生 Schema 的类型枚举使用大写；仅规范化 Schema 节点。
pub(super) fn json_schema(mut value: Value) -> Value {
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
