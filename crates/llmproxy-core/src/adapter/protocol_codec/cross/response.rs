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
        response::{FinishReason, Item as ResponseItem, Response},
    },
    protocol::Protocol,
};
use serde_json::{Map, Value};

/// Responses 可表达终止失败状态；其他完成外壳返回独立失败，由 Gateway 发出 502 错误。
/// 参考：https://developers.openai.com/api/docs/guides/background
pub(super) fn encode_failure(
    protocol: Protocol,
    response: &Response,
    target: &ResponseTarget<'_>,
) -> Result<Conversion<RawResponse>> {
    if protocol != Protocol::OpenAiResponses {
        return Err(crate::adapter::Error::FailedResponse);
    }
    nonempty(target.model, "target.model")?;
    nonempty(target.id, "target.id")?;
    let mut warnings = Vec::new();
    super::apply_diagnostics(
        &response.diagnostics,
        response.source_protocol(),
        protocol,
        &mut warnings,
    )?;
    if !response.items.is_empty() || !response.messages.is_empty() {
        warn(
            &mut warnings,
            response.source_protocol(),
            protocol,
            "output",
            "失败响应的部分输出已丢弃，不能作为完成内容返回",
        );
    }
    let cancelled = response.status == crate::ir::response::Status::Cancelled;
    let error = if cancelled {
        O::Null
    } else {
        O::Value(responses::response::body::ResponseError {
            code: "server_error".into(),
            message: "Provider generation failed".into(),
            misalignment: O::Missing,
            extra: Default::default(),
        })
    };
    Ok(Conversion {
        body: RawResponse::Responses(Box::new(responses::response::Response {
            id: target.id.into(),
            created_at: target.created,
            model: target.model.into(),
            object: "response".into(),
            status: O::Value(if cancelled { "cancelled" } else { "failed" }.into()),
            error,
            usage: response
                .usage
                .as_ref()
                .map(|usage| {
                    let normalized = super::usage::normalize(
                        usage,
                        response.source_protocol(),
                        protocol,
                        &mut warnings,
                    )?;
                    usage_adapter::encode_responses_usage(&normalized, None)
                })
                .transpose()?
                .into(),
            ..Default::default()
        })),
        warnings,
    })
}

pub(super) fn encode_single(
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
    let finish = response.candidates[0].finish_reason;
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
                        PartKind::ServerOutput(output) => {
                            if let Some(text) = super::server_output::text(
                                output,
                                response.source_protocol(),
                                protocol,
                                &part_path,
                                &mut warnings,
                            ) {
                                events.push(OutputEvent::Text(text));
                            }
                        }
                        PartKind::Text(text) => events.push(OutputEvent::Text(text.clone())),
                        PartKind::ToolCall(call) => events.push(OutputEvent::Call(call.clone())),
                        PartKind::Refusal(text) => events.push(OutputEvent::Refusal(text.clone())),
                        PartKind::Reasoning(value) if value.is_string() => {
                            events.push(OutputEvent::Reasoning(value.as_str().unwrap().into()))
                        }
                        PartKind::Media(media) => {
                            if protocol != Protocol::Gemini {
                                return Err(unsupported(
                                    &part_path,
                                    "目标响应没有等价的媒体内容块；不能把媒体伪装成文本",
                                ));
                            }
                            events.push(OutputEvent::Media(media.clone()));
                        }
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
            ResponseItem::Reasoning(text) => events.push(OutputEvent::Reasoning(text.clone())),
            ResponseItem::ServerOutput(output) => {
                if let Some(text) = super::server_output::text(
                    output,
                    response.source_protocol(),
                    protocol,
                    &path,
                    &mut warnings,
                ) {
                    events.push(OutputEvent::Text(text));
                }
            }
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
    if events.is_empty()
        && !response.items.is_empty()
        && !matches!(
            finish,
            FinishReason::Length | FinishReason::Filtered | FinishReason::Refusal
        )
    {
        return Err(unsupported("output", "转换后没有可发送的输出"));
    }
    let has_calls = events
        .iter()
        .any(|event| matches!(event, OutputEvent::Call(_)));
    let refusal = events
        .iter()
        .filter_map(|event| match event {
            OutputEvent::Refusal(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let has_refusal = events
        .iter()
        .any(|event| matches!(event, OutputEvent::Refusal(_)));
    let incomplete = matches!(finish, FinishReason::Length | FinishReason::Filtered);
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
    if (finish == FinishReason::Filtered && protocol == Protocol::AnthropicMessages)
        || ((finish == FinishReason::Refusal || has_refusal) && protocol == Protocol::Gemini)
    {
        warn(
            &mut warnings,
            response.source_protocol(),
            protocol,
            "finish_reason",
            "目标协议没有完全等价的拒绝／过滤分类，保留正文并使用最接近的结束原因",
        );
    }
    if events
        .iter()
        .any(|e| matches!(e, OutputEvent::Reasoning(_)))
    {
        if protocol == Protocol::AnthropicMessages {
            warn(
                &mut warnings,
                response.source_protocol(),
                protocol,
                "reasoning",
                "来源思考没有目标签名，降为可见文本，不能伪造 thinking 签名",
            );
        }
        if protocol == Protocol::OpenAiChat {
            warn(
                &mut warnings,
                response.source_protocol(),
                protocol,
                "reasoning",
                "思考正文写入兼容扩展 reasoning_content",
            );
        }
        if protocol == Protocol::OpenAiResponses {
            warn(
                &mut warnings,
                response.source_protocol(),
                protocol,
                "reasoning",
                "来源可见思考映射为摘要项，不复制不透明签名",
            );
        }
    }
    let usage = response
        .usage
        .as_ref()
        .map(|usage| {
            super::usage::normalize(usage, response.source_protocol(), protocol, &mut warnings)
        })
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
                    finish_reason: match finish {
                        FinishReason::Length => "length",
                        FinishReason::Filtered => "content_filter",
                        _ if has_calls => "tool_calls",
                        _ => "stop",
                    }
                    .into(),
                    logprobs: O::Missing,
                    extra: Default::default(),
                    message: Message {
                        role: chat::response::message::AssistantRole::Assistant,
                        content: if has_text { O::Value(text) } else { O::Null },
                        refusal: if has_refusal {
                            O::Value(refusal)
                        } else {
                            O::Missing
                        },
                        annotations: O::Missing,
                        audio: O::Missing,
                        function_call: O::Missing,
                        tool_calls: if calls.is_empty() {
                            O::Missing
                        } else {
                            O::Value(calls)
                        },
                        extra: {
                            let thinking = events
                                .iter()
                                .filter_map(|e| match e {
                                    OutputEvent::Reasoning(text) => Some(text.as_str()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            if thinking.is_empty() {
                                Default::default()
                            } else {
                                Map::from_iter([(
                                    "reasoning_content".into(),
                                    Value::String(thinking),
                                )])
                            }
                        },
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
                    OutputEvent::Text(text) | OutputEvent::Refusal(text) => {
                        output.push(OutputItem::Message(m::OutputMessage {
                            id: format!("{}-msg-{}", target.id, output.len()),
                            content: vec![if matches!(event, OutputEvent::Refusal(_)) {
                                m::OutputPart::Refusal {
                                    refusal: text.clone(),
                                    extra: Default::default(),
                                }
                            } else {
                                m::OutputPart::OutputText {
                                    text: text.clone(),
                                    annotations: vec![],
                                    logprobs: None,
                                    extra: Default::default(),
                                }
                            }],
                            role: m::AssistantRole::Assistant,
                            status: if incomplete {
                                m::Status::Incomplete
                            } else {
                                m::Status::Completed
                            },
                            r#type: m::MessageType::Message,
                            phase: O::Missing,
                            extra: Default::default(),
                        }))
                    }
                    OutputEvent::Reasoning(text) => {
                        output.push(OutputItem::Other(Map::from_iter([
                            ("type".into(), Value::String("reasoning".into())),
                            (
                                "id".into(),
                                Value::String(format!("{}-reasoning-{}", target.id, output.len())),
                            ),
                            (
                                "summary".into(),
                                serde_json::json!([{"type":"summary_text","text":text}]),
                            ),
                        ])));
                    }
                    OutputEvent::Media(_) => unreachable!("媒体目标已校验"),
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
                            status: O::Value(
                                if incomplete {
                                    "incomplete"
                                } else {
                                    "completed"
                                }
                                .into(),
                            ),
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
                status: O::Value(
                    if incomplete {
                        "incomplete"
                    } else {
                        "completed"
                    }
                    .into(),
                ),
                incomplete_details: if incomplete {
                    O::Value(responses::response::body::IncompleteDetails {
                        reason: if finish == FinishReason::Length {
                            "max_output_tokens"
                        } else {
                            "content_filter"
                        }
                        .into(),
                        extra: Default::default(),
                    })
                } else {
                    O::Null
                },
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
                    OutputEvent::Text(text)
                    | OutputEvent::Refusal(text)
                    | OutputEvent::Reasoning(text) => Block::Text {
                        text: text.clone(),
                        citations: O::Missing,
                        extra: Default::default(),
                    },
                    OutputEvent::Media(_) => unreachable!("媒体目标已校验"),
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
                stop_reason: O::Value(
                    match finish {
                        FinishReason::Length => "max_tokens",
                        FinishReason::Filtered | FinishReason::Refusal => "refusal",
                        _ if has_refusal => "refusal",
                        _ if has_calls => "tool_use",
                        _ => "end_turn",
                    }
                    .into(),
                ),
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
                    OutputEvent::Text(text) | OutputEvent::Refusal(text) => Part {
                        text: O::Value(text.clone()),
                        ..Default::default()
                    },
                    OutputEvent::Reasoning(text) => Part {
                        text: O::Value(text.clone()),
                        thought: O::Value(true),
                        ..Default::default()
                    },
                    OutputEvent::Media(media) => {
                        let mut media = media.clone();
                        media.original = None;
                        match crate::adapter::media::encode(&media, protocol)? {
                            crate::ir::media::OriginalMedia::Gemini(part) => *part,
                            _ => unreachable!(),
                        }
                    }
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
                    finish_reason: O::Value(
                        match finish {
                            FinishReason::Length => "MAX_TOKENS",
                            FinishReason::Filtered => "SAFETY",
                            _ => "STOP",
                        }
                        .into(),
                    ),
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
    Refusal(String),
    Reasoning(String),
    Media(crate::ir::media::Media),
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

/// 候选结构和终止状态只检查 IR，避免回读来源报文。
fn validate_response(response: &Response) -> Result<()> {
    use crate::ir::response::{FinishReason, Status};
    if !matches!(response.status, Status::Completed | Status::Incomplete) {
        return Err(unsupported("status", "响应仍在执行、失败或状态未知"));
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
        FinishReason::Stop
            | FinishReason::ToolCall
            | FinishReason::Length
            | FinishReason::Filtered
            | FinishReason::Refusal
    ) {
        return Err(unsupported(
            "finish_reason",
            "未知结束原因不能伪装为正常结束",
        ));
    }
    Ok(())
}
