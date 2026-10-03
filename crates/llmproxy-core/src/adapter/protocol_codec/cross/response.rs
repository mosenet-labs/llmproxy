//! 非流式响应、函数调用与用量的跨协议映射。

use super::{
    Conversion, ConversionWarning, ResponseTarget, nonempty, unsupported, warn, warn_metadata,
};
use crate::protocol::{
    OptionalNullable as O, Response as RawResponse, chat, gemini, messages, responses,
};
use crate::{
    adapter::{Result, response as usage_adapter},
    ir::{
        message::{PartKind, Role, ToolCall},
        response::{Item as ResponseItem, Response},
        usage::Usage,
    },
    protocol::Protocol,
};
use serde_json::{Map, Value};

pub(in crate::adapter::protocol_codec) fn encode_response(
    protocol: Protocol,
    response: &Response,
    target: &ResponseTarget<'_>,
) -> Result<Conversion<RawResponse>> {
    nonempty(target.model, "target.model")?;
    nonempty(target.id, "target.id")?;
    let mut warnings = Vec::new();
    super::apply_diagnostics(
        &response.diagnostics,
        response.source_protocol(),
        protocol,
        &mut warnings,
    )?;
    validate_response(response)?;
    let mut events = Vec::new();
    let mut serial = 0;
    let mut seen_messages = vec![false; response.messages.len()];
    for (position, item) in response.items.iter().enumerate() {
        let path = format!("items[{position}]");
        match item {
            ResponseItem::Message(index) => {
                let message = response
                    .messages
                    .get(*index)
                    .ok_or_else(|| unsupported(&path, "消息索引无效"))?;
                if std::mem::replace(&mut seen_messages[*index], true) {
                    return Err(unsupported(&path, "消息索引重复"));
                }
                if message.role != Role::Assistant {
                    return Err(unsupported(&path, "响应消息必须来自 assistant"));
                }
                warn_output_metadata(
                    &message.metadata,
                    response.source_protocol(),
                    protocol,
                    &path,
                    &mut warnings,
                )?;
                for (part_index, part) in message.parts.iter().enumerate() {
                    let part_path = format!("{path}.parts[{part_index}]");
                    warn_metadata(
                        &part.metadata,
                        response.source_protocol(),
                        protocol,
                        &part_path,
                        &mut warnings,
                    )?;
                    match &part.kind {
                        PartKind::Text(text) => events.push(OutputEvent::Text(text.clone())),
                        PartKind::ToolCall(call) => events.push(OutputEvent::Call(call.clone())),
                        _ => warn(
                            &mut warnings,
                            response.source_protocol(),
                            protocol,
                            &part_path,
                            "输出片段尚无目标协议映射，已丢弃",
                        ),
                    }
                }
            }
            ResponseItem::ToolCall { call, .. } => events.push(OutputEvent::Call(call.clone())),
            ResponseItem::Opaque(_) => warn(
                &mut warnings,
                response.source_protocol(),
                protocol,
                &path,
                "独立输出项尚无目标协议映射，已丢弃",
            ),
        }
    }
    if seen_messages.iter().any(|seen| !seen) {
        return Err(unsupported("items", "有消息未进入有序输出项"));
    }
    if events.is_empty() {
        return Err(unsupported("output", "转换后没有可发送的输出"));
    }
    let has_calls = events
        .iter()
        .any(|event| matches!(event, OutputEvent::Call(_)));
    let has_text = events
        .iter()
        .any(|event| matches!(event, OutputEvent::Text(_)));
    if has_calls && has_text && protocol == Protocol::OpenAiChat {
        warn(
            &mut warnings,
            response.source_protocol(),
            protocol,
            "output",
            "文本与工具调用的片段顺序合并到一条 Chat 消息",
        );
    }
    let usage = response
        .usage
        .as_ref()
        .map(|usage| normalize_usage(usage, response.source_protocol(), protocol, &mut warnings))
        .transpose()?;
    let body = match protocol {
        Protocol::OpenAiChat => {
            use chat::{
                request::message::{FunctionCall, ToolCall as Call},
                response::{
                    completion::{Choice, Completion},
                    message::Message,
                },
            };
            let text = events
                .iter()
                .filter_map(|event| {
                    if let OutputEvent::Text(text) = event {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut calls = Vec::new();
            for event in &events {
                if let OutputEvent::Call(call) = event {
                    serial += 1;
                    let id = output_call_id(
                        call,
                        serial,
                        response.source_protocol(),
                        protocol,
                        &mut warnings,
                    );
                    calls.push(Call::Function {
                        id,
                        function: FunctionCall {
                            name: call.name.clone(),
                            arguments: call_arguments(call)?,
                            extra: Default::default(),
                        },
                        extra: Default::default(),
                    });
                }
            }
            RawResponse::Chat(Box::new(Completion {
                id: target.id.into(),
                created: target.created,
                model: target.model.into(),
                object: "chat.completion".into(),
                choices: vec![Choice {
                    index: 0,
                    finish_reason: if has_calls { "tool_calls" } else { "stop" }.into(),
                    logprobs: O::Missing,
                    extra: Default::default(),
                    message: Message {
                        role: chat::response::message::AssistantRole::Assistant,
                        content: if has_text { O::Value(text) } else { O::Null },
                        refusal: O::Missing,
                        annotations: O::Missing,
                        audio: O::Missing,
                        function_call: O::Missing,
                        tool_calls: if calls.is_empty() {
                            O::Missing
                        } else {
                            O::Value(calls)
                        },
                        extra: Default::default(),
                    },
                }],
                usage: usage
                    .as_ref()
                    .map(|usage| usage_adapter::encode_chat_usage(usage, None))
                    .transpose()?
                    .into(),
                ..Default::default()
            }))
        }
        Protocol::OpenAiResponses => {
            use responses::{
                function as f,
                request::message as m,
                response::body::{OutputItem, Response},
            };
            let mut output = Vec::new();
            for event in &events {
                match event {
                    OutputEvent::Text(text) => output.push(OutputItem::Message(m::OutputMessage {
                        id: format!("{}-msg-{}", target.id, output.len()),
                        content: vec![m::OutputPart::OutputText {
                            text: text.clone(),
                            annotations: vec![],
                            logprobs: None,
                            extra: Default::default(),
                        }],
                        role: m::AssistantRole::Assistant,
                        status: m::Status::Completed,
                        r#type: m::MessageType::Message,
                        phase: O::Missing,
                        extra: Default::default(),
                    })),
                    OutputEvent::Call(call) => {
                        serial += 1;
                        let id = output_call_id(
                            call,
                            serial,
                            response.source_protocol(),
                            protocol,
                            &mut warnings,
                        );
                        output.push(OutputItem::FunctionCall(f::Call {
                            r#type: f::CallType::FunctionCall,
                            id: O::Value(format!("{}-call-{}", target.id, serial)),
                            call_id: O::Value(id),
                            name: call.name.clone(),
                            arguments: call_arguments(call)?,
                            status: O::Value("completed".into()),
                            extra: Default::default(),
                        }));
                    }
                }
            }
            RawResponse::Responses(Box::new(Response {
                id: target.id.into(),
                created_at: target.created,
                model: target.model.into(),
                object: "response".into(),
                status: O::Value("completed".into()),
                output,
                usage: usage
                    .as_ref()
                    .map(|usage| usage_adapter::encode_responses_usage(usage, None))
                    .transpose()?
                    .into(),
                ..Default::default()
            }))
        }
        Protocol::AnthropicMessages => {
            use messages::response::message::{
                AssistantRole, ContentBlock, KnownContentBlock as Block, Message, MessageType,
            };
            let mut content = Vec::new();
            for event in &events {
                content.push(ContentBlock::Known(match event {
                    OutputEvent::Text(text) => Block::Text {
                        text: text.clone(),
                        citations: O::Missing,
                        extra: Default::default(),
                    },
                    OutputEvent::Call(call) => {
                        serial += 1;
                        let id = output_call_id(
                            call,
                            serial,
                            response.source_protocol(),
                            protocol,
                            &mut warnings,
                        );
                        Block::ToolUse {
                            id,
                            name: call.name.clone(),
                            input: call_arguments_object(call)?,
                            caller: O::Missing,
                            extra: Default::default(),
                        }
                    }
                }));
            }
            let usage = usage
                .as_ref()
                .ok_or_else(|| unsupported("usage", "Messages 响应必须包含用量"))?;
            RawResponse::Messages(Box::new(Message {
                r#type: MessageType::Message,
                id: target.id.into(),
                container: O::Missing,
                content,
                diagnostics: O::Missing,
                model: target.model.into(),
                role: AssistantRole::Assistant,
                stop_details: O::Missing,
                stop_reason: O::Value(if has_calls { "tool_use" } else { "end_turn" }.into()),
                stop_sequence: O::Missing,
                usage: usage_adapter::encode_messages_usage(usage, None)?,
                extra: Default::default(),
            }))
        }
        Protocol::Gemini => {
            use gemini::{
                request::message::{FunctionCall, Message, Part, Role},
                response::body::{Candidate, Response},
            };
            let mut parts = Vec::new();
            for event in &events {
                parts.push(match event {
                    OutputEvent::Text(text) => Part {
                        text: O::Value(text.clone()),
                        ..Default::default()
                    },
                    OutputEvent::Call(call) => {
                        serial += 1;
                        let id = output_call_id(
                            call,
                            serial,
                            response.source_protocol(),
                            protocol,
                            &mut warnings,
                        );
                        Part {
                            function_call: O::Value(FunctionCall {
                                id: O::Value(id),
                                name: call.name.clone(),
                                args: O::Value(call_arguments_object(call)?),
                                extra: Default::default(),
                            }),
                            ..Default::default()
                        }
                    }
                });
            }
            RawResponse::Gemini(Box::new(Response {
                response_id: O::Value(target.id.into()),
                model_version: O::Value(target.model.into()),
                candidates: O::Value(vec![Candidate {
                    content: O::Value(Message {
                        role: Some(Role::Model),
                        parts,
                        extra: Default::default(),
                    }),
                    finish_reason: O::Value("STOP".into()),
                    ..Default::default()
                }]),
                usage_metadata: usage
                    .as_ref()
                    .map(|usage| usage_adapter::encode_gemini_usage(usage, None))
                    .into(),
                ..Default::default()
            }))
        }
    };
    Ok(Conversion { body, warnings })
}

enum OutputEvent {
    Text(String),
    Call(ToolCall),
}

fn call_arguments(call: &ToolCall) -> Result<String> {
    match &call.arguments {
        Value::String(raw) => Ok(raw.clone()),
        value => Ok(serde_json::to_string(value)?),
    }
}

fn call_arguments_object(call: &ToolCall) -> Result<Map<String, Value>> {
    let value = if let Value::String(raw) = &call.arguments {
        serde_json::from_str(raw)
            .map_err(|_| unsupported("tool.arguments", "工具参数不是合法 JSON"))?
    } else {
        call.arguments.clone()
    };
    if !value.is_object() {
        return Err(unsupported("tool.arguments", "工具参数必须是 JSON 对象"));
    }
    Ok(value.as_object().expect("参数对象已验证").clone())
}

fn output_call_id(
    call: &ToolCall,
    serial: usize,
    source: Protocol,
    target: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> String {
    call.id.clone().unwrap_or_else(|| {
        warn(
            warnings,
            source,
            target,
            "output.tool_call.id",
            "工具调用缺少 ID，已生成配对 ID",
        );
        format!("call_{serial}")
    })
}

fn warn_output_metadata(
    metadata: &Map<String, Value>,
    source: Protocol,
    target: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let mut cleaned = metadata.clone();
    if let Some(wire) = cleaned
        .get_mut("_llmproxy_wire")
        .and_then(Value::as_object_mut)
        && let Some(extra) = wire.get_mut("extra").and_then(Value::as_object_mut)
    {
        for key in [
            "type",
            "id",
            "model",
            "status",
            "stop_reason",
            "stop_sequence",
            "usage",
        ] {
            extra.remove(key);
        }
    }
    warn_metadata(&cleaned, source, target, path, warnings)
}

fn normalize_usage(
    usage: &Usage,
    source: Protocol,
    target: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<Usage> {
    if usage.cache != Default::default()
        || usage.input_details != Default::default()
        || usage.output_details != Default::default()
    {
        warn(
            warnings,
            source,
            target,
            "usage.details",
            "缓存和模态细分用量未映射，已保留总用量",
        );
    }
    let (Some(input), Some(output), Some(total)) =
        (usage.input_tokens, usage.output_tokens, usage.total_tokens)
    else {
        return Err(unsupported("usage", "缺少输入、输出或总词元数"));
    };
    if input.checked_add(output) != Some(total) {
        return Err(unsupported("usage.total_tokens", "与输入输出词元数不一致"));
    }
    Ok(Usage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        total_tokens: Some(total),
        ..Usage::default()
    })
}

/// 候选结构和终止状态只检查 IR，避免回读来源报文。
fn validate_response(response: &Response) -> Result<()> {
    use crate::ir::response::{FinishReason, Status};
    if response.status != Status::Completed {
        return Err(unsupported("status", "仅支持正常完成的响应"));
    }
    if response.candidates.len() != 1 {
        return Err(unsupported("candidates/choices", "只支持单候选"));
    }
    let candidate = &response.candidates[0];
    if candidate.index != 0 || candidate.items != (0..response.items.len()).collect::<Vec<_>>() {
        return Err(unsupported("candidates", "候选序号或输出项边界无效"));
    }
    if !matches!(
        candidate.finish_reason,
        FinishReason::Stop | FinishReason::ToolCall
    ) {
        return Err(unsupported(
            "finish_reason",
            "仅支持正常完成或工具调用的响应",
        ));
    }
    Ok(())
}
