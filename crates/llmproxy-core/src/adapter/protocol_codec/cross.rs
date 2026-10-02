//! 跨协议的首批公共语义：非流式、单候选、纯文本。

use serde_json::{Map, Value, json};

use crate::{
    adapter::{Error, Result, response as usage_adapter, wire},
    ir::{
        message::{Part, PartKind, Role},
        request::Request,
        response::Response,
        usage::Usage,
    },
    protocol::{Protocol, chat, gemini, messages, responses},
};

/// 目标请求所在路由的模型及可选输出上限。
#[derive(Clone, Copy, Debug)]
pub struct RequestTarget<'a> {
    /// Provider 侧模型 ID；Gemini 由 Gateway 放入 URL。
    pub model: &'a str,
    /// 来源未指定上限时由路由明确提供；不会覆盖来源上限。
    pub max_output_tokens: Option<u64>,
}

/// 客户端可见响应外壳；这些值不能从另一 Provider 的协议外壳假定得出。
#[derive(Clone, Copy, Debug)]
pub struct ResponseTarget<'a> {
    /// 客户端可见模型 ID。
    pub model: &'a str,
    /// 本次响应 ID。
    pub id: &'a str,
    /// Unix 秒级创建时间；Gemini 和 Messages 正文不使用。
    pub created: i64,
}

pub(super) fn encode_request(
    protocol: Protocol,
    request: &Request,
    target: &RequestTarget<'_>,
) -> Result<Value> {
    nonempty(target.model, "target.model")?;
    if request.cache != Default::default() {
        return Err(unsupported("cache", "显式缓存设置尚未跨协议转换"));
    }
    let source = request.source.clone().into_body()?;
    let allowed = match request.source_protocol() {
        Protocol::OpenAiChat => &["model", "messages", "max_completion_tokens", "stream"][..],
        Protocol::OpenAiResponses => &["model", "input", "max_output_tokens", "stream"][..],
        Protocol::AnthropicMessages => &["model", "messages", "max_tokens", "stream"][..],
        Protocol::Gemini => &["contents", "generationConfig"][..],
    };
    check_keys(&source, allowed, "request")?;
    if request.source_protocol() == Protocol::OpenAiResponses
        && source["input"]
            .as_array()
            .is_none_or(|items| items.len() != request.messages.len())
    {
        return Err(unsupported("input", "包含非消息输入项或未提供显式历史"));
    }
    let limit = source_limit(request.source_protocol(), &source)?;
    if let (Some(source_limit), Some(target_limit)) = (limit, target.max_output_tokens)
        && source_limit != target_limit
    {
        return Err(unsupported("max_output_tokens", "路由上限与来源上限冲突"));
    }
    let limit = limit.or(target.max_output_tokens);
    let mut items = Vec::with_capacity(request.messages.len());
    for (index, message) in request.messages.iter().enumerate() {
        let path = format!("messages[{index}]");
        check_metadata(&message.metadata, request.source_protocol(), &path, false)?;
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            _ => {
                return Err(unsupported(
                    &format!("{path}.role"),
                    "只支持 user/assistant",
                ));
            }
        };
        let text = one_text(&message.parts, request.source_protocol(), &path, false)?;
        items.push(match protocol {
            Protocol::Gemini => json!({"role": if role == "assistant" { "model" } else { role }, "parts":[{"text":text}]}),
            _ => json!({"role":role,"content":text}),
        });
    }
    if items.is_empty() {
        return Err(unsupported("messages", "显式历史不能为空"));
    }
    let body = match protocol {
        Protocol::OpenAiChat => {
            let mut body = json!({"model":target.model,"messages":items});
            if let Some(limit) = limit {
                body["max_completion_tokens"] = json!(limit);
            }
            body
        }
        Protocol::OpenAiResponses => {
            let mut body = json!({"model":target.model,"input":items});
            if let Some(limit) = limit {
                body["max_output_tokens"] = json!(limit);
            }
            body
        }
        Protocol::AnthropicMessages => {
            json!({"model":target.model,"messages":items,"max_tokens":limit.ok_or_else(|| unsupported("max_tokens", "Messages 请求必须明确输出上限"))?})
        }
        Protocol::Gemini => {
            let mut body = json!({"contents":items});
            if let Some(limit) = limit {
                body["generationConfig"] = json!({"maxOutputTokens":limit});
            }
            body
        }
    };
    typed_request(protocol, body)
}

pub(super) fn encode_response(
    protocol: Protocol,
    response: &Response,
    target: &ResponseTarget<'_>,
) -> Result<Value> {
    nonempty(target.model, "target.model")?;
    nonempty(target.id, "target.id")?;
    let source = response.source.clone().into_body()?;
    validate_response_source(response.source_protocol(), &source)?;
    if response.messages.len() != 1 {
        return Err(unsupported("messages", "响应必须恰好包含一个候选消息"));
    }
    let message = &response.messages[0];
    if message.role != Role::Assistant {
        return Err(unsupported("messages[0].role", "响应只支持 assistant"));
    }
    check_metadata(
        &message.metadata,
        response.source_protocol(),
        "messages[0]",
        true,
    )?;
    let text = one_text(
        &message.parts,
        response.source_protocol(),
        "messages[0]",
        true,
    )?;
    let usage = response.usage.as_ref().map(validate_usage).transpose()?;
    let usage_value = match (protocol, usage) {
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
            json!({"id":target.id,"created":target.created,"model":target.model,"object":"chat.completion","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":text}}]})
        }
        Protocol::OpenAiResponses => {
            json!({"id":target.id,"created_at":target.created,"model":target.model,"object":"response","status":"completed","output":[{"id":format!("{}-msg",target.id),"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}]})
        }
        Protocol::AnthropicMessages => {
            json!({"type":"message","id":target.id,"model":target.model,"role":"assistant","content":[{"type":"text","text":text}],"stop_reason":"end_turn"})
        }
        Protocol::Gemini => {
            json!({"responseId":target.id,"modelVersion":target.model,"candidates":[{"content":{"role":"model","parts":[{"text":text}]},"finishReason":"STOP"}]})
        }
    };
    if let Some(usage) = usage_value {
        body[match protocol {
            Protocol::Gemini => "usageMetadata",
            _ => "usage",
        }] = usage;
    }
    typed_response(protocol, body)
}

fn source_limit(protocol: Protocol, body: &Value) -> Result<Option<u64>> {
    let stream = body.get("stream");
    if stream.is_some_and(|value| value != false) {
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
            if let Some(config) = config {
                check_keys(config, &["maxOutputTokens"], "generationConfig")?;
            }
            config.and_then(|value| value.get("maxOutputTokens"))
        }
    };
    optional_u64(value, "max_output_tokens")
}

fn optional_u64(value: Option<&Value>, path: &str) -> Result<Option<u64>> {
    value
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| unsupported(path, "必须是非负整数"))
        })
        .transpose()
}

fn validate_response_source(protocol: Protocol, body: &Value) -> Result<()> {
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
    check_keys(body, allowed, "response")?;
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
        check_keys(usage, allowed_usage, "usage")?;
    }
    if body.pointer(finish_path).and_then(Value::as_str) != Some(expected) {
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
            check_keys(
                &choices[0],
                &["index", "finish_reason", "message", "logprobs"],
                "choices[0]",
            )?;
            if choices[0]["index"] != 0 {
                return Err(unsupported("choices[0].index", "单候选序号必须为零"));
            }
            if choices[0].get("logprobs").is_some_and(|v| !v.is_null()) {
                return Err(unsupported("choices[0].logprobs", "尚未转换"));
            }
        }
        Protocol::OpenAiResponses => {
            if body["object"] != "response" {
                return Err(unsupported("object", "不是 Responses 完成响应"));
            }
            if body["output"].as_array().is_none_or(|v| v.len() != 1) {
                return Err(unsupported("output", "只支持单个消息输出项"));
            }
        }
        Protocol::AnthropicMessages => {
            if body.get("stop_sequence").is_some_and(|v| !v.is_null()) {
                return Err(unsupported("stop_sequence", "自定义停止序列尚未转换"));
            }
        }
        Protocol::Gemini => {
            let candidates = body["candidates"]
                .as_array()
                .ok_or_else(|| unsupported("candidates", "必须是数组"))?;
            if candidates.len() != 1 {
                return Err(unsupported("candidates", "只支持单候选"));
            }
            check_keys(
                &candidates[0],
                &["content", "finishReason", "index"],
                "candidates[0]",
            )?;
            if candidates[0].get("index").is_some_and(|v| v != 0) {
                return Err(unsupported("candidates[0].index", "单候选序号必须为零"));
            }
        }
    }
    Ok(())
}

fn validate_usage(usage: &Usage) -> Result<&Usage> {
    if usage.cache != Default::default()
        || usage.input_details.text_tokens.is_some()
        || usage.input_details.audio_tokens.is_some()
        || usage.input_details.image_tokens.is_some()
        || usage.input_details.video_tokens.is_some()
        || usage.input_details.document_tokens.is_some()
        || usage.input_details.tool_tokens.is_some()
        || usage.output_details != Default::default()
    {
        return Err(unsupported("usage", "缓存或细分用量尚未跨协议转换"));
    }
    let (Some(input), Some(output), Some(total)) =
        (usage.input_tokens, usage.output_tokens, usage.total_tokens)
    else {
        return Err(unsupported("usage", "缺少输入、输出或总词元数"));
    };
    if input.checked_add(output) != Some(total) {
        return Err(unsupported("usage.total_tokens", "与输入输出词元数不一致"));
    }
    if usage
        .input_details
        .uncached_tokens
        .is_some_and(|count| count != input)
    {
        return Err(unsupported(
            "usage.input_details.uncached_tokens",
            "与输入词元数不一致",
        ));
    }
    Ok(usage)
}

fn one_text<'a>(parts: &'a [Part], source: Protocol, path: &str, output: bool) -> Result<&'a str> {
    if parts.len() != 1 {
        return Err(unsupported(&format!("{path}.parts"), "只支持单段文本"));
    }
    check_metadata(
        &parts[0].metadata,
        source,
        &format!("{path}.parts[0]"),
        output,
    )?;
    match &parts[0].kind {
        PartKind::Text(text) => Ok(text),
        _ => Err(unsupported(&format!("{path}.parts[0].kind"), "只支持文本")),
    }
}

fn check_metadata(
    metadata: &Map<String, Value>,
    source: Protocol,
    path: &str,
    output: bool,
) -> Result<()> {
    if metadata.contains_key("_llmproxy_wire") && wire::form(metadata, source).is_none() {
        return Err(unsupported(path, "消息来源协议标记不匹配"));
    }
    let mut extra = wire::extra(metadata, source);
    if output
        && source == Protocol::OpenAiResponses
        && !path.contains(".parts[")
        && (extra.remove("type") != Some(json!("message"))
            || extra.remove("status") != Some(json!("completed"))
            || extra.remove("id").is_none_or(|id| !id.is_string()))
    {
        return Err(unsupported(path, "Responses 输出消息外壳不完整"));
    }
    if output && source == Protocol::AnthropicMessages && !path.contains(".parts[") {
        for key in [
            "type",
            "id",
            "model",
            "stop_reason",
            "stop_sequence",
            "usage",
        ] {
            extra.remove(key);
        }
    }
    if output && source == Protocol::OpenAiResponses && extra.get("annotations") == Some(&json!([]))
    {
        extra.remove("annotations");
    }
    if !extra.is_empty() || metadata.keys().any(|key| key != "_llmproxy_wire") {
        return Err(unsupported(path, "来源消息含未映射的附加字段"));
    }
    Ok(())
}

fn check_keys(value: &Value, allowed: &[&str], path: &str) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported(path, "必须是 JSON 对象"))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(unsupported(&format!("{path}.{key}"), "字段尚未跨协议转换"));
        }
    }
    Ok(())
}

fn nonempty(value: &str, path: &str) -> Result<()> {
    if value.is_empty() {
        Err(unsupported(path, "不能为空"))
    } else {
        Ok(())
    }
}

fn unsupported(path: &str, reason: &str) -> Error {
    Error::Unsupported(format!("{path}: {reason}"))
}

fn typed_request(protocol: Protocol, body: Value) -> Result<Value> {
    Ok(match protocol {
        Protocol::OpenAiChat => {
            serde_json::to_value(serde_json::from_value::<chat::request::Request>(body)?)?
        }
        Protocol::OpenAiResponses => {
            serde_json::to_value(serde_json::from_value::<responses::request::Request>(body)?)?
        }
        Protocol::AnthropicMessages => {
            serde_json::to_value(serde_json::from_value::<messages::request::Request>(body)?)?
        }
        Protocol::Gemini => {
            serde_json::to_value(serde_json::from_value::<gemini::request::Request>(body)?)?
        }
    })
}

fn typed_response(protocol: Protocol, body: Value) -> Result<Value> {
    Ok(match protocol {
        Protocol::OpenAiChat => {
            serde_json::to_value(serde_json::from_value::<chat::response::Completion>(body)?)?
        }
        Protocol::OpenAiResponses => serde_json::to_value(serde_json::from_value::<
            responses::response::Response,
        >(body)?)?,
        Protocol::AnthropicMessages => {
            serde_json::to_value(serde_json::from_value::<messages::response::Message>(body)?)?
        }
        Protocol::Gemini => {
            serde_json::to_value(serde_json::from_value::<gemini::response::Response>(body)?)?
        }
    })
}
