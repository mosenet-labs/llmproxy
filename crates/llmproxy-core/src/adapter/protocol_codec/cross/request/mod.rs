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
mod cache;
mod items;
mod results;
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
    super::cache::validate(request)?;
    let mut warnings = Vec::new();
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
    for (index, instruction) in request.instructions.iter().enumerate() {
        high.push((
            instruction.role,
            instruction.text.clone(),
            "instructions".to_owned(),
            breakpoint(
                request,
                &crate::ir::cache::CacheLocation::Instruction(index),
            ),
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
                    for (part_index, part) in message.parts.iter().enumerate() {
                        let text = text_parts(
                            std::slice::from_ref(part),
                            request.source_protocol(),
                            protocol,
                            &path,
                            &mut warnings,
                        )?;
                        high.push((
                            role,
                            text,
                            path.clone(),
                            breakpoint(
                                request,
                                &crate::ir::cache::CacheLocation::Message {
                                    message: *index,
                                    part: part_index,
                                },
                            ),
                        ));
                    }
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
                    let mut metadata = part.metadata.clone();
                    let signature = if request.source_protocol() == Protocol::Gemini
                        && protocol == Protocol::Gemini
                        && matches!(part.kind, PartKind::ToolCall(_))
                    {
                        let mut extra = crate::adapter::wire::extra(&metadata, Protocol::Gemini);
                        let signature = crate::adapter::wire::take(&mut extra, "thoughtSignature")?;
                        if let Some(form) =
                            crate::adapter::wire::form(&part.metadata, Protocol::Gemini)
                        {
                            crate::adapter::wire::save(
                                &mut metadata,
                                Protocol::Gemini,
                                form,
                                extra,
                            );
                        }
                        signature
                    } else {
                        crate::protocol::OptionalNullable::Missing
                    };
                    if !matches!(part.kind, PartKind::ToolResult(_)) {
                        warn_metadata(
                            &metadata,
                            request.source_protocol(),
                            protocol,
                            &part_path,
                            &mut warnings,
                        )?;
                    }
                    match &part.kind {
                        PartKind::ServerOutput(output) => {
                            if let Some(text) = super::server_output::text(
                                output,
                                request.source_protocol(),
                                protocol,
                                &part_path,
                                &mut warnings,
                            ) {
                                emit_text(
                                    protocol,
                                    &mut items,
                                    role,
                                    &text,
                                    request.source_protocol(),
                                    &part_path,
                                    &mut warnings,
                                )?;
                            }
                        }
                        PartKind::Text(text) => emit_text(
                            protocol,
                            &mut items,
                            role,
                            text,
                            request.source_protocol(),
                            &part_path,
                            &mut warnings,
                        )?,
                        PartKind::Reasoning(value) => {
                            if let Some(text) = value.as_str() {
                                warn(
                                    &mut warnings,
                                    request.source_protocol(),
                                    protocol,
                                    &part_path,
                                    "可见思考历史降为 assistant 文本，签名不跨协议复制",
                                );
                                emit_text(
                                    protocol,
                                    &mut items,
                                    role,
                                    text,
                                    request.source_protocol(),
                                    &part_path,
                                    &mut warnings,
                                )?;
                            } else {
                                warn(
                                    &mut warnings,
                                    request.source_protocol(),
                                    protocol,
                                    &part_path,
                                    "不透明思考数据仅在来源协议保留",
                                );
                            }
                        }
                        PartKind::Media(media) => {
                            if let crate::ir::media::MediaSource::Text(text) = &media.source
                                && media.kind == crate::ir::media::MediaKind::File
                                && protocol != Protocol::AnthropicMessages
                            {
                                warn(
                                    &mut warnings,
                                    request.source_protocol(),
                                    protocol,
                                    &part_path,
                                    "文本文档降为文本，保留正文；文档标题、边界与文档引用能力丢失",
                                );
                                emit_text(
                                    protocol,
                                    &mut items,
                                    role,
                                    text,
                                    request.source_protocol(),
                                    &part_path,
                                    &mut warnings,
                                )?;
                            } else {
                                items.media(role, media)?;
                            }
                        }
                        PartKind::Refusal(text) => {
                            warn(
                                &mut warnings,
                                request.source_protocol(),
                                protocol,
                                &part_path,
                                "历史拒绝说明映射为文本",
                            );
                            emit_text(
                                protocol,
                                &mut items,
                                role,
                                text,
                                request.source_protocol(),
                                &part_path,
                                &mut warnings,
                            )?;
                        }
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
                            items.call_signature(signature);
                        }
                        PartKind::ToolResult(result) => emit_result(
                            protocol,
                            &mut items,
                            (result, Some(&part.metadata)),
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
                    if let Some(point) = breakpoint(
                        request,
                        &crate::ir::cache::CacheLocation::Message {
                            message: *index,
                            part: part_index,
                        },
                    ) {
                        if !matches!(&part.kind, PartKind::Opaque(_) | PartKind::ServerOutput(_))
                            && !matches!(&part.kind, PartKind::Reasoning(v) if !v.is_string())
                        {
                            mark(
                                &mut items,
                                point,
                                request.source_protocol(),
                                protocol,
                                &part_path,
                                &mut warnings,
                            );
                        } else {
                            warn(
                                &mut warnings,
                                request.source_protocol(),
                                protocol,
                                &part_path,
                                "内容已丢弃，缓存断点同步丢弃",
                            );
                        }
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
                    (result, None),
                    &mut calls,
                    request.source_protocol(),
                    &path,
                    &mut warnings,
                )?;
            }
            RequestItem::Reasoning(text) => {
                seen_dialogue = true;
                warn(
                    &mut warnings,
                    request.source_protocol(),
                    protocol,
                    &path,
                    "历史思考摘要降为 assistant 文本",
                );
                emit_text(
                    protocol,
                    &mut items,
                    Role::Assistant,
                    text,
                    request.source_protocol(),
                    &path,
                    &mut warnings,
                )?;
            }
            RequestItem::ServerOutput(output) => {
                if let Some(text) = super::server_output::text(
                    output,
                    request.source_protocol(),
                    protocol,
                    &path,
                    &mut warnings,
                ) {
                    emit_text(
                        protocol,
                        &mut items,
                        Role::Assistant,
                        &text,
                        request.source_protocol(),
                        &path,
                        &mut warnings,
                    )?;
                }
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
    let mut system_points = Vec::new();
    // Responses 顶层指令不能标记断点；需要断点时整组改为 system 输入，保持顺序。
    let instructions_as_messages = protocol == Protocol::OpenAiResponses
        && high.iter().any(|(_, _, _, point)| point.is_some());
    for (role, text, path, point) in high {
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
                if let Some(point) = point {
                    mark(
                        &mut prefix,
                        point,
                        request.source_protocol(),
                        protocol,
                        &path,
                        &mut warnings,
                    );
                }
            }
            Protocol::OpenAiResponses if path != "instructions" || instructions_as_messages => {
                if path == "instructions" {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &path,
                        "为保留缓存断点和指令顺序，将顶层指令映射为 system 输入项",
                    );
                }
                prefix.text(role, &text)?;
                if let Some(point) = point {
                    mark(
                        &mut prefix,
                        point,
                        request.source_protocol(),
                        protocol,
                        &path,
                        &mut warnings,
                    );
                }
            }
            Protocol::OpenAiResponses => {
                system.push(text);
                if point.is_some() {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &path,
                        "Responses 顶层 instructions 不支持内容块断点，已丢弃断点",
                    );
                }
            }
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
                let control = point.map(|point| {
                    super::cache::control(point, request.source_protocol(), protocol, &mut warnings)
                });
                if point.is_some() && protocol == Protocol::Gemini {
                    warn(
                        &mut warnings,
                        request.source_protocol(),
                        protocol,
                        &path,
                        "Gemini 生成请求没有内容块缓存断点，已丢弃断点",
                    );
                }
                system_points.push(control);
                system.push(text);
            }
        }
    }
    prefix.extend(items);
    let items = prefix;
    if items.is_empty() {
        return Err(unsupported("messages", "转换后没有可发送的输入"));
    }
    let mut body = items.finish(target.model, system.clone());
    if let crate::protocol::Request::Messages(body) = &mut body
        && system_points.iter().any(Option::is_some)
    {
        body.system = crate::protocol::OptionalNullable::Value(
            crate::protocol::messages::request::body::SystemPrompt::Parts(
                system
                    .into_iter()
                    .zip(system_points)
                    .map(|(text, cache_control)| {
                        crate::protocol::messages::request::body::SystemText {
                            text,
                            cache_control: cache_control.into(),
                            citations: crate::protocol::OptionalNullable::Missing,
                            r#type: "text".into(),
                            extra: Default::default(),
                        }
                    })
                    .collect(),
            ),
        );
    }
    super::tools::encode_tools(
        request.source_protocol(),
        &mut body,
        &request.tools,
        &mut warnings,
    )?;
    super::cache::tools(request, &mut body, &mut warnings)?;
    super::native::encode(request, &mut body, &mut warnings)?;
    let mut generation = request.generation.clone();
    generation.max_output_tokens = limit;
    super::generation::encode(
        protocol,
        request.source_protocol(),
        &generation,
        &mut body,
        &mut warnings,
    )?;
    super::cache::encode(
        request.source_protocol(),
        &mut body,
        &request.cache,
        &mut warnings,
    )?;
    super::cache::validate_messages(&body)?;
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
    result: (&ToolResult, Option<&Map<String, Value>>),
    calls: &mut ToolState,
    source: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let (result, metadata) = result;
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
    results::emit(items, id.clone(), name, result, metadata, source, warnings)?;
    calls.pending.remove(&id);
    Ok(())
}

/// 断点通过规范化位置关联，不依赖来源报文的 JSON 路径。
fn breakpoint<'a>(
    request: &'a Request,
    location: &crate::ir::cache::CacheLocation,
) -> Option<&'a crate::ir::cache::Breakpoint> {
    request
        .cache_breakpoints
        .iter()
        .find(|point| &point.location == location)
}
/// 目标没有断点位置时明确记录降级。
fn mark(
    items: &mut Items,
    point: &crate::ir::cache::Breakpoint,
    source: Protocol,
    target: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) {
    let control = super::cache::control(point, source, target, warnings);
    if !items.cache_breakpoint(control) {
        warn(
            warnings,
            source,
            target,
            path,
            "目标内容项没有缓存断点字段，已丢弃断点",
        );
    }
}
