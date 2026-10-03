//! 请求角色、顶层指令与客户端工具的跨协议映射。

use super::{
    Conversion, ConversionWarning, RequestTarget, nonempty, unsupported, warn, warn_metadata,
};
use crate::{
    adapter::Result,
    ir::{
        message::{Part, PartKind, Role, ToolCall, ToolResult},
        request::{Item as RequestItem, Request},
    },
    protocol::Protocol,
};
use serde_json::{Map, Value};
mod items;
use items::Items;
use std::collections::{HashMap, HashSet};

pub(in crate::adapter::protocol_codec) fn encode_request(
    protocol: Protocol,
    request: &Request,
    target: &RequestTarget<'_>,
) -> Result<Conversion<crate::protocol::Request>> {
    if protocol != Protocol::Gemini {
        nonempty(target.model, "target.model")?;
    }
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
    super::apply_diagnostics(
        &request.diagnostics,
        request.source_protocol(),
        protocol,
        &mut warnings,
    )?;
    if request.generation.stream {
        return Err(unsupported("stream", "仅支持非流式请求"));
    }
    let limit = request.generation.max_output_tokens;
    if let (Some(source_limit), Some(target_limit)) = (limit, target.max_output_tokens)
        && source_limit != target_limit
    {
        return Err(unsupported("max_output_tokens", "路由上限与来源上限冲突"));
    }
    let limit = limit.or(target.max_output_tokens);
    let mut items = Items::new(protocol);
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
    let mut prefix = Items::new(protocol);
    for (role, text, path) in high {
        match protocol {
            Protocol::OpenAiChat => {
                if path == "instructions" {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &path,
                        "顶层 instructions 近似映射为 Chat system 消息",
                    );
                }
                prefix.text(role, &text)?;
            }
            Protocol::OpenAiResponses if path != "instructions" => {
                prefix.text(role, &text)?;
            }
            Protocol::OpenAiResponses => system.push(text),
            Protocol::AnthropicMessages | Protocol::Gemini => {
                if role == Role::Developer {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &path,
                        "developer 与 system 的优先级合并",
                    );
                }
                system.push(text);
            }
        }
    }
    prefix.extend(items);
    let items = prefix;
    if items.is_empty() {
        return Err(unsupported("messages", "转换后没有可发送的输入"));
    }
    let mut body = items.finish(target.model, system);
    super::tools::encode_tools(
        request.source_protocol(),
        &mut body,
        &request.tools,
        &mut warnings,
    )?;
    let mut generation = request.generation.clone();
    generation.max_output_tokens = limit;
    super::generation::encode(
        protocol,
        request.source_protocol(),
        &generation,
        &mut body,
        &mut warnings,
    )?;
    Ok(Conversion { body, warnings })
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
    items: &mut Items,
    role: Role,
    text: &str,
    source: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    if items.text(role, text)? {
        warn(
            warnings,
            source,
            target,
            path,
            "工具调用后的文本已合并到同一条 assistant 消息",
        );
    }
    Ok(())
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
    items: &mut Items,
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
    items.call(
        id,
        &call.name,
        args.as_object().expect("参数对象已验证").clone(),
    )?;
    Ok(())
}

fn emit_result(
    target: Protocol,
    items: &mut Items,
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
    let response = result
        .content
        .as_object()
        .cloned()
        .unwrap_or_else(|| Map::from_iter([("result".into(), result.content.clone())]));
    items.result(id.clone(), name, content, response);
    calls.pending.remove(&id);
    Ok(())
}
