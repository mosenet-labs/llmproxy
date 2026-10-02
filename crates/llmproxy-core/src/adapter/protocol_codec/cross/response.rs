//! 非流式响应、函数调用与用量的跨协议映射。

use super::{
    Conversion, ConversionWarning, ResponseTarget, nonempty, typed_response, unsupported, warn,
    warn_keys, warn_metadata,
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
use serde_json::{Map, Value, json};

pub(in crate::adapter::protocol_codec) fn encode_response(
    protocol: Protocol,
    response: &Response,
    target: &ResponseTarget<'_>,
) -> Result<Conversion> {
    nonempty(target.model, "target.model")?;
    nonempty(target.id, "target.id")?;
    let source = response.source.clone().into_body()?;
    let mut warnings = Vec::new();
    validate_response_source(response.source_protocol(), protocol, &source, &mut warnings)?;
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
    let usage_value = match (protocol, usage.as_ref()) {
        (Protocol::OpenAiChat, Some(value)) => Some(serde_json::to_value(
            usage_adapter::encode_chat_usage(value, None)?,
        )?),
        (Protocol::OpenAiResponses, Some(value)) => Some(serde_json::to_value(
            usage_adapter::encode_responses_usage(value, None)?,
        )?),
        (Protocol::AnthropicMessages, Some(value)) => Some(serde_json::to_value(
            usage_adapter::encode_messages_usage(value, None)?,
        )?),
        (Protocol::Gemini, Some(value)) => Some(serde_json::to_value(
            usage_adapter::encode_gemini_usage(value, None),
        )?),
        (Protocol::AnthropicMessages, None) => {
            return Err(unsupported("usage", "Messages 响应必须包含用量"));
        }
        (_, None) => None,
    };
    let mut body = match protocol {
        Protocol::OpenAiChat => {
            let text = events
                .iter()
                .filter_map(|event| match event {
                    OutputEvent::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut message = json!({"role":"assistant","content":if has_text { json!(text) } else { Value::Null }});
            if has_calls {
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
                        calls.push(json!({"id":id,"type":"function","function":{"name":call.name,"arguments":call_arguments(call)?}}));
                    }
                }
                message["tool_calls"] = json!(calls);
            }
            json!({"id":target.id,"created":target.created,"model":target.model,"object":"chat.completion","choices":[{"index":0,"finish_reason":if has_calls { "tool_calls" } else { "stop" },"message":message}]})
        }
        Protocol::OpenAiResponses => {
            let mut output = Vec::new();
            for event in &events {
                match event {
                OutputEvent::Text(text) => output.push(json!({"id":format!("{}-msg-{}",target.id,output.len()),"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]})),
                OutputEvent::Call(call) => {
                    serial += 1;
                    let id = output_call_id(call, serial, response.source_protocol(), protocol, &mut warnings);
                    output.push(json!({"type":"function_call","id":format!("{}-call-{}",target.id,serial),"call_id":id,"name":call.name,"arguments":call_arguments(call)?,"status":"completed"}));
                }
            }
            }
            json!({"id":target.id,"created_at":target.created,"model":target.model,"object":"response","status":"completed","output":output})
        }
        Protocol::AnthropicMessages => {
            let mut content = Vec::new();
            for event in &events {
                match event {
                    OutputEvent::Text(text) => content.push(json!({"type":"text","text":text})),
                    OutputEvent::Call(call) => {
                        serial += 1;
                        let id = output_call_id(
                            call,
                            serial,
                            response.source_protocol(),
                            protocol,
                            &mut warnings,
                        );
                        content.push(json!({"type":"tool_use","id":id,"name":call.name,"input":call_arguments_object(call)?}));
                    }
                }
            }
            json!({"type":"message","id":target.id,"model":target.model,"role":"assistant","content":content,"stop_reason":if has_calls { "tool_use" } else { "end_turn" }})
        }
        Protocol::Gemini => {
            let mut parts = Vec::new();
            for event in &events {
                match event {
                    OutputEvent::Text(text) => parts.push(json!({"text":text})),
                    OutputEvent::Call(call) => {
                        serial += 1;
                        let id = output_call_id(
                            call,
                            serial,
                            response.source_protocol(),
                            protocol,
                            &mut warnings,
                        );
                        parts.push(json!({"functionCall":{"id":id,"name":call.name,"args":call_arguments_object(call)?}}));
                    }
                }
            }
            json!({"responseId":target.id,"modelVersion":target.model,"candidates":[{"content":{"role":"model","parts":parts},"finishReason":"STOP"}]})
        }
    };
    if let Some(usage) = usage_value {
        body[match protocol {
            Protocol::Gemini => "usageMetadata",
            _ => "usage",
        }] = usage;
    }
    Ok(Conversion {
        body: typed_response(protocol, body)?,
        warnings,
    })
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

fn call_arguments_object(call: &ToolCall) -> Result<Value> {
    let value = if let Value::String(raw) = &call.arguments {
        serde_json::from_str(raw)
            .map_err(|_| unsupported("tool.arguments", "工具参数不是合法 JSON"))?
    } else {
        call.arguments.clone()
    };
    if !value.is_object() {
        return Err(unsupported("tool.arguments", "工具参数必须是 JSON 对象"));
    }
    Ok(value)
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

fn validate_response_source(
    protocol: Protocol,
    target: Protocol,
    body: &Value,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let (allowed, finish_path, expected) = match protocol {
        Protocol::OpenAiChat => (
            &["id", "created", "model", "object", "choices", "usage"][..],
            "/choices/0/finish_reason",
            "stop",
        ),
        Protocol::OpenAiResponses => (
            &[
                "id",
                "created_at",
                "model",
                "object",
                "output",
                "status",
                "usage",
            ][..],
            "/status",
            "completed",
        ),
        Protocol::AnthropicMessages => (
            &[
                "type",
                "id",
                "model",
                "role",
                "content",
                "stop_reason",
                "stop_sequence",
                "usage",
            ][..],
            "/stop_reason",
            "end_turn",
        ),
        Protocol::Gemini => (
            &["candidates", "usageMetadata", "modelVersion", "responseId"][..],
            "/candidates/0/finishReason",
            "STOP",
        ),
    };
    warn_keys(body, allowed, "response", protocol, target, warnings)?;
    let usage = body.get(if protocol == Protocol::Gemini {
        "usageMetadata"
    } else {
        "usage"
    });
    if let Some(usage) = usage.filter(|usage| !usage.is_null()) {
        let allowed_usage = match protocol {
            Protocol::OpenAiChat => &["prompt_tokens", "completion_tokens", "total_tokens"][..],
            Protocol::OpenAiResponses => &["input_tokens", "output_tokens", "total_tokens"][..],
            Protocol::AnthropicMessages => &["input_tokens", "output_tokens"][..],
            Protocol::Gemini => &[
                "promptTokenCount",
                "candidatesTokenCount",
                "totalTokenCount",
            ][..],
        };
        warn_keys(usage, allowed_usage, "usage", protocol, target, warnings)?;
    }
    let finish = body.pointer(finish_path).and_then(Value::as_str);
    let tool_finish = match protocol {
        Protocol::OpenAiChat => Some("tool_calls"),
        Protocol::AnthropicMessages => Some("tool_use"),
        _ => None,
    };
    if finish != Some(expected) && finish != tool_finish {
        return Err(unsupported(finish_path, "仅支持正常完成的响应"));
    }
    match protocol {
        Protocol::OpenAiChat => {
            if body["object"] != "chat.completion" {
                return Err(unsupported("object", "不是 Chat 完成响应"));
            }
            let choices = body["choices"]
                .as_array()
                .ok_or_else(|| unsupported("choices", "必须是数组"))?;
            if choices.len() != 1 {
                return Err(unsupported("choices", "只支持单候选"));
            }
            warn_keys(
                &choices[0],
                &["index", "finish_reason", "message", "logprobs"],
                "choices[0]",
                protocol,
                target,
                warnings,
            )?;
            if choices[0]["index"] != 0 {
                return Err(unsupported("choices[0].index", "单候选序号必须为零"));
            }
            if choices[0].get("logprobs").is_some_and(|v| !v.is_null()) {
                warn(
                    warnings,
                    protocol,
                    target,
                    "choices[0].logprobs",
                    "对数概率未映射，已丢弃",
                );
            }
        }
        Protocol::OpenAiResponses => {
            if body["object"] != "response" {
                return Err(unsupported("object", "不是 Responses 完成响应"));
            }
            if body["output"].as_array().is_none_or(Vec::is_empty) {
                return Err(unsupported("output", "输出项不能为空"));
            }
            if let Some(output) = body["output"].as_array() {
                for (index, item) in output.iter().enumerate() {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        if item.get("status").is_some_and(|value| value != "completed") {
                            return Err(unsupported(
                                &format!("output[{index}].status"),
                                "函数调用尚未完成",
                            ));
                        }
                        warn_keys(
                            item,
                            &["type", "id", "call_id", "name", "arguments", "status"],
                            &format!("output[{index}]"),
                            protocol,
                            target,
                            warnings,
                        )?;
                    }
                }
            }
        }
        Protocol::AnthropicMessages => {
            if body.get("stop_sequence").is_some_and(|v| !v.is_null()) {
                warn(
                    warnings,
                    protocol,
                    target,
                    "stop_sequence",
                    "自定义停止序列未映射，已丢弃",
                );
            }
        }
        Protocol::Gemini => {
            let candidates = body["candidates"]
                .as_array()
                .ok_or_else(|| unsupported("candidates", "必须是数组"))?;
            if candidates.len() != 1 {
                return Err(unsupported("candidates", "只支持单候选"));
            }
            warn_keys(
                &candidates[0],
                &["content", "finishReason", "index"],
                "candidates[0]",
                protocol,
                target,
                warnings,
            )?;
            if candidates[0].get("index").is_some_and(|v| v != 0) {
                return Err(unsupported("candidates[0].index", "单候选序号必须为零"));
            }
        }
    }
    Ok(())
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
