//! 请求角色、顶层指令与客户端工具的跨协议映射。

use super::{
    Conversion, ConversionWarning, RequestTarget, nonempty, typed_request, unsupported, warn,
    warn_keys, warn_metadata,
};
use crate::{
    adapter::Result,
    ir::{
        message::{Part, PartKind, Role, ToolCall, ToolResult},
        request::{Item as RequestItem, Request},
    },
    protocol::Protocol,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

pub(in crate::adapter::protocol_codec) fn encode_request(
    protocol: Protocol,
    request: &Request,
    target: &RequestTarget<'_>,
) -> Result<Conversion> {
    nonempty(target.model, "target.model")?;
    let mut warnings = Vec::new();
    if request.cache != Default::default() {
        warn(
            &mut warnings,
            request.source_protocol(),
            protocol,
            "cache",
            "目标请求未保留来源缓存设置",
        );
    }
    let source = request.source.clone().into_body()?;
    let allowed = match request.source_protocol() {
        Protocol::OpenAiChat => &[
            "model",
            "messages",
            "tools",
            "max_completion_tokens",
            "stream",
        ][..],
        Protocol::OpenAiResponses => &[
            "model",
            "input",
            "instructions",
            "tools",
            "max_output_tokens",
            "stream",
        ][..],
        Protocol::AnthropicMessages => &[
            "model",
            "messages",
            "system",
            "tools",
            "max_tokens",
            "stream",
        ][..],
        Protocol::Gemini => &["contents", "systemInstruction", "tools", "generationConfig"][..],
    };
    warn_keys(
        &source,
        allowed,
        "request",
        request.source_protocol(),
        protocol,
        &mut warnings,
    )?;
    warn_instruction_details(request.source_protocol(), protocol, &source, &mut warnings)?;
    if request.source_protocol() == Protocol::OpenAiResponses
        && let Some(input) = source.get("input").and_then(Value::as_array)
    {
        for (index, item) in input.iter().enumerate() {
            let allowed = match item.get("type").and_then(Value::as_str) {
                Some("function_call") => {
                    Some(&["type", "id", "call_id", "name", "arguments", "status"][..])
                }
                Some("function_call_output") => Some(&["type", "call_id", "output", "status"][..]),
                _ => None,
            };
            if let Some(allowed) = allowed {
                if item.get("status").is_some_and(|value| value != "completed") {
                    return Err(unsupported(
                        &format!("input[{index}].status"),
                        "历史工具项尚未完成",
                    ));
                }
                warn_keys(
                    item,
                    allowed,
                    &format!("input[{index}]"),
                    request.source_protocol(),
                    protocol,
                    &mut warnings,
                )?;
            }
        }
    }
    let limit = source_limit(request.source_protocol(), &source)?;
    if let Some(config) = source
        .get("generationConfig")
        .filter(|value| !value.is_null())
    {
        warn_keys(
            config,
            &["maxOutputTokens"],
            "generationConfig",
            request.source_protocol(),
            protocol,
            &mut warnings,
        )?;
    }
    if let (Some(source_limit), Some(target_limit)) = (limit, target.max_output_tokens)
        && source_limit != target_limit
    {
        return Err(unsupported("max_output_tokens", "路由上限与来源上限冲突"));
    }
    let limit = limit.or(target.max_output_tokens);
    let mut items = Vec::new();
    let mut high = Vec::new();
    for instruction in &request.instructions {
        high.push((
            instruction.role,
            instruction.text.clone(),
            "instructions".to_owned(),
        ));
    }
    let mut seen_dialogue = false;
    let mut calls = ToolState::default();
    let mut seen_messages = vec![false; request.messages.len()];
    for (position, item) in request.items.iter().enumerate() {
        let path = format!("items[{position}]");
        match item {
            RequestItem::Message(index) => {
                let message = request
                    .messages
                    .get(*index)
                    .ok_or_else(|| unsupported(&path, "消息索引无效"))?;
                if std::mem::replace(&mut seen_messages[*index], true) {
                    return Err(unsupported(&path, "消息索引重复"));
                }
                warn_metadata(
                    &message.metadata,
                    request.source_protocol(),
                    protocol,
                    &path,
                    &mut warnings,
                )?;
                let role = message.role;
                if matches!(role, Role::System | Role::Developer) && !seen_dialogue {
                    let text = text_parts(
                        &message.parts,
                        request.source_protocol(),
                        protocol,
                        &path,
                        &mut warnings,
                    )?;
                    high.push((role, text, path));
                    continue;
                }
                if role == Role::Unspecified && protocol != Protocol::Gemini {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &format!("{path}.role"),
                        "目标协议不支持未指定角色，已丢弃消息",
                    );
                    continue;
                }
                seen_dialogue = true;
                let role = if matches!(role, Role::System | Role::Developer)
                    && matches!(protocol, Protocol::AnthropicMessages | Protocol::Gemini)
                {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &format!("{path}.role"),
                        "中途高优先级指令降为 user",
                    );
                    Role::User
                } else {
                    role
                };
                for (part_index, part) in message.parts.iter().enumerate() {
                    let part_path = format!("{path}.parts[{part_index}]");
                    warn_metadata(
                        &part.metadata,
                        request.source_protocol(),
                        protocol,
                        &part_path,
                        &mut warnings,
                    )?;
                    match &part.kind {
                        PartKind::Text(text) => emit_text(
                            protocol,
                            &mut items,
                            role,
                            text,
                            request.source_protocol(),
                            &part_path,
                            &mut warnings,
                        )?,
                        PartKind::ToolCall(call) => {
                            if role != Role::Assistant {
                                return Err(unsupported(&part_path, "工具调用必须来自 assistant"));
                            }
                            emit_call(
                                protocol,
                                &mut items,
                                call,
                                &mut calls,
                                request.source_protocol(),
                                &part_path,
                                &mut warnings,
                            )?;
                        }
                        PartKind::ToolResult(result) => emit_result(
                            protocol,
                            &mut items,
                            result,
                            &mut calls,
                            request.source_protocol(),
                            &part_path,
                            &mut warnings,
                        )?,
                        _ => warn(
                            &mut warnings,
                            request.source_protocol(),
                            protocol,
                            &part_path,
                            "内容片段尚无目标协议映射，已丢弃",
                        ),
                    }
                }
            }
            RequestItem::ToolCall { call, .. } => {
                seen_dialogue = true;
                emit_call(
                    protocol,
                    &mut items,
                    call,
                    &mut calls,
                    request.source_protocol(),
                    &path,
                    &mut warnings,
                )?;
            }
            RequestItem::ToolResult(result) => {
                seen_dialogue = true;
                emit_result(
                    protocol,
                    &mut items,
                    result,
                    &mut calls,
                    request.source_protocol(),
                    &path,
                    &mut warnings,
                )?;
            }
            RequestItem::Opaque(_) => warn(
                &mut warnings,
                request.source_protocol(),
                protocol,
                &path,
                "独立输入项尚无目标协议映射，已丢弃",
            ),
        }
    }
    if seen_messages.iter().any(|seen| !seen) {
        return Err(unsupported("items", "有消息未进入有序输入项"));
    }
    let mut system = Vec::new();
    let mut prefix = Vec::new();
    for (role, text, path) in high {
        match protocol {
            Protocol::OpenAiChat => {
                if path == "instructions" { warn(&mut warnings, request.source_protocol(), protocol, &path, "顶层 instructions 近似映射为 Chat system 消息"); }
                prefix.push(json!({"role": if role == Role::Developer { "developer" } else { "system" }, "content":text}));
            }
            Protocol::OpenAiResponses if path != "instructions" => prefix.push(json!({"role": if role == Role::Developer { "developer" } else { "system" }, "content":text})),
            Protocol::OpenAiResponses => system.push(text),
            Protocol::AnthropicMessages | Protocol::Gemini => {
                if role == Role::Developer { warn(&mut warnings, request.source_protocol(), protocol, &path, "developer 与 system 的优先级合并"); }
                system.push(text);
            }
        }
    }
    prefix.extend(items);
    let items = prefix;
    if items.is_empty() {
        return Err(unsupported("messages", "转换后没有可发送的输入"));
    }
    let mut body = match protocol {
        Protocol::OpenAiChat => {
            let mut body = json!({"model":target.model,"messages":items});
            if let Some(limit) = limit {
                body["max_completion_tokens"] = json!(limit);
            }
            body
        }
        Protocol::OpenAiResponses => {
            let mut body = json!({"model":target.model,"input":items});
            if !system.is_empty() {
                body["instructions"] = json!(system.join("\n"));
            }
            if let Some(limit) = limit {
                body["max_output_tokens"] = json!(limit);
            }
            body
        }
        Protocol::AnthropicMessages => {
            let mut body = json!({"model":target.model,"messages":items,"max_tokens":limit.ok_or_else(|| unsupported("max_tokens", "Messages 请求必须明确输出上限"))?});
            if !system.is_empty() {
                body["system"] = json!(system.join("\n"));
            }
            body
        }
        Protocol::Gemini => {
            let mut body = json!({"contents":items});
            if !system.is_empty() {
                body["systemInstruction"] = json!({"parts":[{"text":system.join("\n")}]});
            }
            if let Some(limit) = limit {
                body["generationConfig"] = json!({"maxOutputTokens":limit});
            }
            body
        }
    };
    let tools = tool_definitions(request.source_protocol(), protocol, &source, &mut warnings)?;
    if !tools.is_empty() {
        body["tools"] = match protocol {
            Protocol::Gemini => json!([{"functionDeclarations":tools}]),
            _ => json!(tools),
        };
    }
    Ok(Conversion {
        body: typed_request(protocol, body)?,
        warnings,
    })
}

fn warn_instruction_details(
    source: Protocol,
    target: Protocol,
    body: &Value,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    match source {
        Protocol::AnthropicMessages => {
            if let Some(parts) = body.get("system").and_then(Value::as_array) {
                for (index, part) in parts.iter().enumerate() {
                    warn_keys(
                        part,
                        &["type", "text"],
                        &format!("system[{index}]"),
                        source,
                        target,
                        warnings,
                    )?;
                }
            }
        }
        Protocol::Gemini => {
            if let Some(instruction) = body
                .get("systemInstruction")
                .filter(|value| !value.is_null())
            {
                warn_keys(
                    instruction,
                    &["parts"],
                    "systemInstruction",
                    source,
                    target,
                    warnings,
                )?;
                if let Some(parts) = instruction.get("parts").and_then(Value::as_array) {
                    for (index, part) in parts.iter().enumerate() {
                        warn_keys(
                            part,
                            &["text"],
                            &format!("systemInstruction.parts[{index}]"),
                            source,
                            target,
                            warnings,
                        )?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

struct ToolDefinition {
    name: String,
    description: Option<String>,
    parameters: Value,
    gemini_native_schema: bool,
    strict: Option<bool>,
}

/// 仅映射客户端函数声明；服务端内置工具仍交由原协议处理。
fn tool_definitions(
    source_protocol: Protocol,
    target: Protocol,
    body: &Value,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<Vec<Value>> {
    let Some(raw_tools) = body.get("tools").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let raw_tools = raw_tools
        .as_array()
        .ok_or_else(|| unsupported("tools", "必须是数组"))?;
    let mut definitions = Vec::new();
    for (index, raw) in raw_tools.iter().enumerate() {
        let path = format!("tools[{index}]");
        let sources = if source_protocol == Protocol::Gemini {
            if let Some(functions) = raw.get("functionDeclarations").and_then(Value::as_array) {
                warn_keys(
                    raw,
                    &["functionDeclarations"],
                    &path,
                    source_protocol,
                    target,
                    warnings,
                )?;
                functions.iter().collect::<Vec<_>>()
            } else {
                warn(
                    warnings,
                    source_protocol,
                    target,
                    &path,
                    "服务端工具声明尚无跨协议映射，已丢弃",
                );
                continue;
            }
        } else {
            vec![raw]
        };
        if source_protocol == Protocol::OpenAiChat {
            warn_keys(
                raw,
                &["type", "function"],
                &path,
                source_protocol,
                target,
                warnings,
            )?;
        }
        for (function_index, function) in sources.into_iter().enumerate() {
            let function_path = format!("{path}.function[{function_index}]");
            if source_protocol != Protocol::Gemini
                && function
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind != "function")
            {
                warn(
                    warnings,
                    source_protocol,
                    target,
                    &function_path,
                    "非客户端函数声明已丢弃",
                );
                continue;
            }
            let function = if source_protocol == Protocol::OpenAiChat {
                function.get("function").unwrap_or(function)
            } else {
                function
            };
            let allowed = match source_protocol {
                Protocol::OpenAiChat | Protocol::OpenAiResponses => {
                    &["type", "name", "description", "parameters", "strict"][..]
                }
                Protocol::AnthropicMessages => &["name", "description", "input_schema"][..],
                Protocol::Gemini => {
                    &["name", "description", "parameters", "parametersJsonSchema"][..]
                }
            };
            warn_keys(
                function,
                allowed,
                &function_path,
                source_protocol,
                target,
                warnings,
            )?;
            let Some(name) = function.get("name").and_then(Value::as_str) else {
                return Err(unsupported(&function_path, "函数声明缺少 name"));
            };
            let (parameters, gemini_native_schema) = if source_protocol == Protocol::Gemini {
                if let Some(schema) = function.get("parametersJsonSchema") {
                    (schema.clone(), false)
                } else if let Some(schema) = function.get("parameters") {
                    (schema.clone(), true)
                } else {
                    return Err(unsupported(&function_path, "函数声明缺少参数 Schema"));
                }
            } else {
                (
                    function
                        .get(if source_protocol == Protocol::AnthropicMessages {
                            "input_schema"
                        } else {
                            "parameters"
                        })
                        .cloned()
                        .ok_or_else(|| unsupported(&function_path, "函数声明缺少参数 Schema"))?,
                    false,
                )
            };
            let definition = ToolDefinition {
                name: name.into(),
                description: function
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parameters,
                gemini_native_schema,
                strict: function.get("strict").and_then(Value::as_bool),
            };
            if !definition.parameters.is_object() {
                return Err(unsupported(&function_path, "函数参数 Schema 必须是对象"));
            }
            if definition.strict.is_some()
                && matches!(target, Protocol::AnthropicMessages | Protocol::Gemini)
            {
                warn(
                    warnings,
                    source_protocol,
                    target,
                    &format!("{function_path}.strict"),
                    "严格 Schema 模式无对应字段，已丢弃",
                );
            }
            definitions.push(match target {
                Protocol::OpenAiChat => {
                    let mut function = json!({"name":definition.name,"parameters":json_schema(definition.parameters, definition.gemini_native_schema)});
                    if let Some(description) = definition.description { function["description"] = json!(description); }
                    if let Some(strict) = definition.strict { function["strict"] = json!(strict); }
                    json!({"type":"function","function":function})
                }
                Protocol::OpenAiResponses => {
                    let mut function = json!({"type":"function","name":definition.name,"parameters":json_schema(definition.parameters, definition.gemini_native_schema)});
                    if let Some(description) = definition.description { function["description"] = json!(description); }
                    if let Some(strict) = definition.strict { function["strict"] = json!(strict); }
                    function
                }
                Protocol::AnthropicMessages => {
                    let mut function = json!({"name":definition.name,"input_schema":json_schema(definition.parameters, definition.gemini_native_schema)});
                    if let Some(description) = definition.description { function["description"] = json!(description); }
                    function
                }
                Protocol::Gemini => {
                    let description = definition.description.ok_or_else(|| unsupported(&function_path, "Gemini 函数声明必须有 description"))?;
                    let mut function = json!({"name":definition.name,"description":description});
                    function[if definition.gemini_native_schema { "parameters" } else { "parametersJsonSchema" }] = definition.parameters;
                    function
                }
            });
        }
    }
    Ok(definitions)
}

/// Gemini 原生 Schema 使用大写枚举；JSON Schema 目标使用标准小写类型。
fn json_schema(mut value: Value, gemini_native: bool) -> Value {
    if !gemini_native {
        return value;
    }
    fn normalize(value: &mut Value) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(kind)) = object.get_mut("type") {
                    *kind = kind.to_ascii_lowercase();
                }
                for child in object.values_mut() {
                    normalize(child);
                }
            }
            Value::Array(items) => {
                for child in items {
                    normalize(child);
                }
            }
            _ => {}
        }
    }
    normalize(&mut value);
    value
}

fn text_parts(
    parts: &[Part],
    source: Protocol,
    target: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<String> {
    let mut text = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let part_path = format!("{path}.parts[{index}]");
        warn_metadata(&part.metadata, source, target, &part_path, warnings)?;
        match &part.kind {
            PartKind::Text(value) => text.push(value.as_str()),
            _ => warn(warnings, source, target, &part_path, "非文本指令片段已丢弃"),
        }
    }
    Ok(text.join("\n"))
}

fn emit_text(
    target: Protocol,
    items: &mut Vec<Value>,
    role: Role,
    text: &str,
    source: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let role_name = match role {
        Role::System => "system",
        Role::Developer => "developer",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Unspecified if target == Protocol::Gemini => "",
        _ => return Err(unsupported(path, "文本片段角色无法转换")),
    };
    if role == Role::Assistant {
        match target {
            Protocol::OpenAiChat => {
                if let Some(last) = items.last_mut().filter(|item| {
                    item.get("role") == Some(&json!("assistant"))
                        && item.get("tool_calls").is_some()
                }) {
                    let previous = last.get("content").and_then(Value::as_str).unwrap_or("");
                    last["content"] = json!(if previous.is_empty() {
                        text.to_owned()
                    } else {
                        format!("{previous}\n{text}")
                    });
                    warn(
                        warnings,
                        source,
                        target,
                        path,
                        "工具调用后的文本已合并到同一条 assistant 消息",
                    );
                    return Ok(());
                }
            }
            Protocol::AnthropicMessages
                if append_block(
                    items,
                    "assistant",
                    "content",
                    json!({"type":"text","text":text}),
                ) =>
            {
                return Ok(());
            }
            Protocol::Gemini if append_block(items, "model", "parts", json!({"text":text})) => {
                return Ok(());
            }
            _ => {}
        }
    }
    let value = match target {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => {
            json!({"role":role_name,"content":text})
        }
        Protocol::AnthropicMessages => json!({"role":role_name,"content":text}),
        Protocol::Gemini if role == Role::Unspecified => json!({"parts":[{"text":text}]}),
        Protocol::Gemini => {
            json!({"role":if role == Role::Assistant { "model" } else { "user" },"parts":[{"text":text}]})
        }
    };
    items.push(value);
    Ok(())
}

/// 在目标协议允许时，将连续内容并入上一条同角色消息。
fn append_block(items: &mut [Value], role: &str, field: &str, block: Value) -> bool {
    let Some(last) = items
        .last_mut()
        .filter(|item| item.get("role") == Some(&json!(role)))
    else {
        return false;
    };
    let Some(content) = last.get_mut(field) else {
        return false;
    };
    match content {
        Value::Array(parts) => parts.push(block),
        Value::String(text) if field == "content" => {
            let previous = std::mem::take(text);
            *content = json!([{"type":"text","text":previous}, block]);
        }
        _ => return false,
    }
    true
}

#[derive(Default)]
struct ToolState {
    /// 尚未收到结果的调用，供按 ID 或名称配对。
    pending: HashMap<String, String>,
    /// 已使用 ID 不能在同一历史中重复，包含已经配对完成的调用。
    used_ids: HashSet<String>,
    serial: usize,
}

fn call_id(
    call: &ToolCall,
    serial: usize,
    source: Protocol,
    target: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> String {
    if let Some(id) = &call.id {
        return id.clone();
    }
    warn(
        warnings,
        source,
        target,
        path,
        "工具调用缺少 ID，已生成本次请求内的配对 ID",
    );
    format!("call_{serial}")
}

fn emit_call(
    target: Protocol,
    items: &mut Vec<Value>,
    call: &ToolCall,
    calls: &mut ToolState,
    source: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    calls.serial += 1;
    while calls.used_ids.contains(&format!("call_{}", calls.serial)) {
        calls.serial += 1;
    }
    let id = call_id(call, calls.serial, source, target, path, warnings);
    if !calls.used_ids.insert(id.clone()) {
        return Err(unsupported(path, "工具调用 ID 重复"));
    }
    calls.pending.insert(id.clone(), call.name.clone());
    let args = if let Value::String(raw) = &call.arguments {
        serde_json::from_str::<Value>(raw)
            .map_err(|_| unsupported(path, "工具参数不是合法 JSON"))?
    } else {
        call.arguments.clone()
    };
    if !args.is_object() {
        return Err(unsupported(path, "工具参数必须是 JSON 对象"));
    }
    match target {
        Protocol::OpenAiChat => {
            let block = json!({"id":id,"type":"function","function":{"name":call.name,"arguments":serde_json::to_string(&args)?}});
            if let Some(last) = items.last_mut().filter(|item| item.get("role") == Some(&json!("assistant"))) {
                if let Some(calls) = last.get_mut("tool_calls").and_then(Value::as_array_mut) {
                    calls.push(block);
                    return Ok(());
                }
                if last.get("content").is_some() {
                    last["tool_calls"] = json!([block]);
                    return Ok(());
                }
            }
            items.push(json!({"role":"assistant","content":null,"tool_calls":[block]}));
        }
        Protocol::OpenAiResponses => items.push(json!({"type":"function_call","call_id":id,"name":call.name,"arguments":serde_json::to_string(&args)?})),
        Protocol::AnthropicMessages => {
            let block = json!({"type":"tool_use","id":id,"name":call.name,"input":args});
            if !append_block(items, "assistant", "content", block.clone()) {
                items.push(json!({"role":"assistant","content":[block]}));
            }
        }
        Protocol::Gemini => {
            let block = json!({"functionCall":{"id":id,"name":call.name,"args":args}});
            if !append_block(items, "model", "parts", block.clone()) {
                items.push(json!({"role":"model","parts":[block]}));
            }
        }
    }
    Ok(())
}

fn emit_result(
    target: Protocol,
    items: &mut Vec<Value>,
    result: &ToolResult,
    calls: &mut ToolState,
    source: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let id = match &result.id {
        Some(id) if calls.pending.contains_key(id) => id.clone(),
        Some(_) => return Err(unsupported(path, "工具结果找不到对应调用 ID")),
        None => {
            let Some(name) = &result.name else {
                return Err(unsupported(path, "工具结果缺少调用 ID 和名称"));
            };
            let matches = calls
                .pending
                .iter()
                .filter(|(_, value)| *value == name)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(unsupported(path, "无法通过名称唯一匹配工具调用"));
            }
            warn(
                warnings,
                source,
                target,
                path,
                "工具结果按名称匹配到调用 ID",
            );
            matches[0].0.clone()
        }
    };
    let name = calls.pending.get(&id).expect("调用 ID 已验证").clone();
    if result.name.as_ref().is_some_and(|actual| actual != &name) {
        return Err(unsupported(path, "工具结果名称与调用不一致"));
    }
    if (target == Protocol::Gemini && !result.content.is_object())
        || (target != Protocol::Gemini && !result.content.is_string())
    {
        warn(
            warnings,
            source,
            target,
            path,
            if target == Protocol::Gemini {
                "非对象工具结果已包装为 result 对象"
            } else {
                "非文本工具结果已序列化为 JSON 文本"
            },
        );
    }
    let content = if let Some(text) = result.content.as_str() {
        text.to_owned()
    } else {
        serde_json::to_string(&result.content)?
    };
    match target {
        Protocol::OpenAiChat => {
            items.push(json!({"role":"tool","tool_call_id":id,"content":content}))
        }
        Protocol::OpenAiResponses => {
            items.push(json!({"type":"function_call_output","call_id":id,"output":content}))
        }
        Protocol::AnthropicMessages => {
            let block = json!({"type":"tool_result","tool_use_id":id,"content":content});
            if !append_block(items, "user", "content", block.clone()) {
                items.push(json!({"role":"user","content":[block]}));
            }
        }
        Protocol::Gemini => {
            let response = result
                .content
                .as_object()
                .cloned()
                .unwrap_or_else(|| Map::from_iter([("result".into(), result.content.clone())]));
            let block = json!({"functionResponse":{"id":id,"name":name,"response":response}});
            if !append_block(items, "user", "parts", block.clone()) {
                items.push(json!({"role":"user","parts":[block]}));
            }
        }
    }
    calls.pending.remove(&id);
    Ok(())
}

fn source_limit(protocol: Protocol, body: &Value) -> Result<Option<u64>> {
    let stream = body.get("stream");
    if stream.is_some_and(|value| !value.is_null() && value != false) {
        return Err(unsupported("stream", "仅支持非流式请求"));
    }
    let value = match protocol {
        Protocol::OpenAiChat => {
            return optional_u64(body.get("max_completion_tokens"), "max_completion_tokens");
        }
        Protocol::OpenAiResponses => body.get("max_output_tokens"),
        Protocol::AnthropicMessages => body.get("max_tokens"),
        Protocol::Gemini => {
            let config = body.get("generationConfig");
            config.and_then(|value| value.get("maxOutputTokens"))
        }
    };
    optional_u64(value, "max_output_tokens")
}

fn optional_u64(value: Option<&Value>, path: &str) -> Result<Option<u64>> {
    value
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| unsupported(path, "必须是非负整数"))
        })
        .transpose()
}
