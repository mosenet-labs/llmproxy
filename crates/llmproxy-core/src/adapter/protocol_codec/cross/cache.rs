//! 请求级缓存只转换配置语义；私有缓存引用不跨 Provider 搬运。
use super::{ConversionWarning, unsupported, warn};
use crate::{
    adapter::{Result, request},
    ir::cache::CacheSettings,
    protocol::{Protocol, Request},
};

/// 缓存键／TTL 可映射的部分沿用专用缓存编码器，不会静默删除缓存引用承载的上下文。
pub(super) fn encode(
    source: Protocol,
    body: &mut Request,
    cache: &CacheSettings,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = body.protocol();
    let mut cache = cache.clone();
    if cache.reference.is_some() && !(source == Protocol::Gemini && target == Protocol::Gemini) {
        return Err(unsupported(
            "cache.reference",
            "缓存引用包含来源 Provider 的上下文，必须先物化为完整输入",
        ));
    }
    if matches!(target, Protocol::OpenAiChat | Protocol::OpenAiResponses) {
        if cache.mode.as_deref() == Some("ephemeral") {
            cache.mode = Some("implicit".into());
            warn(
                warnings,
                source,
                target,
                "cache.mode",
                "请求级临时缓存近似映射为隐式缓存；命中策略由目标 Provider 决定",
            );
        }
    } else if target == Protocol::AnthropicMessages {
        if cache.key.take().is_some() {
            warn(
                warnings,
                source,
                target,
                "cache.key",
                "Messages 无缓存分组键，已丢弃",
            );
        }
        if cache.retention.take().is_some() {
            warn(
                warnings,
                source,
                target,
                "cache.retention",
                "Messages 无对应缓存保留策略，已丢弃",
            );
        }
        if cache.mode.is_some() {
            cache.mode = Some("ephemeral".into());
        }
        if cache
            .ttl
            .as_deref()
            .is_some_and(|ttl| !matches!(ttl, "5m" | "1h"))
        {
            cache.ttl = None;
            warn(
                warnings,
                source,
                target,
                "cache.ttl",
                "目标仅支持 5m／1h，使用目标默认 TTL",
            );
        }
    } else {
        for (path, present) in [
            ("cache.key", cache.key.is_some()),
            ("cache.mode", cache.mode.is_some()),
            ("cache.ttl", cache.ttl.is_some()),
            ("cache.retention", cache.retention.is_some()),
        ] {
            if present {
                warn(
                    warnings,
                    source,
                    target,
                    path,
                    "Gemini 缓存配置位于独立的缓存创建 API，生成请求不携带此参数",
                );
            }
        }
    }
    match body {
        Request::Chat(b) => request::encode_chat_cache(b, &cache),
        Request::Responses(b) => request::encode_responses_cache(b, &cache),
        Request::Messages(b) => request::encode_messages_cache(b, &cache),
        Request::Gemini(b) => request::encode_gemini_cache(b, &cache),
    }
    Ok(())
}

/// TTL 不能转换为词元数或全局保留策略，无法表达时仅丢弃 TTL。
pub(super) fn control(
    point: &crate::ir::cache::Breakpoint,
    source: Protocol,
    target: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> crate::protocol::messages::request::cache::CacheControl {
    let mut ttl = point.ttl.clone();
    if ttl.is_some()
        && (target != Protocol::AnthropicMessages
            || ttl.as_deref().is_some_and(|v| !matches!(v, "5m" | "1h")))
    {
        warn(
            warnings,
            source,
            target,
            "cache.breakpoint.ttl",
            "目标断点不能表达来源 TTL，保留断点并采用目标默认 TTL",
        );
        ttl = None;
    }
    crate::protocol::messages::request::cache::CacheControl {
        r#type: "ephemeral".into(),
        ttl: ttl.into(),
        extra: Default::default(),
    }
}
/// 函数缓存断点保持在原函数声明末尾；目标没有对应字段则记录警告。
pub(super) fn tools(
    request: &crate::ir::request::Request,
    body: &mut Request,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let target = body.protocol();
    for point in &request.cache_breakpoints {
        if let crate::ir::cache::CacheLocation::Tool(index) = point.location {
            if index >= request.tools.len() {
                return Err(unsupported("cache.breakpoint.tool", "工具断点索引越界"));
            }
            if let Request::Messages(body) = body {
                let control = control(point, request.source_protocol(), target, warnings);
                if let Some(crate::protocol::messages::request::tool::Tool::Function(tool)) =
                    match &mut body.tools {
                        crate::protocol::OptionalNullable::Value(tools) => tools.get_mut(index),
                        _ => None,
                    }
                {
                    tool.extra
                        .insert("cache_control".into(), serde_json::to_value(control)?);
                }
            } else {
                warn(
                    warnings,
                    request.source_protocol(),
                    target,
                    "cache.breakpoint.tool",
                    "目标函数声明没有缓存断点字段，已丢弃断点",
                );
            }
        }
    }
    Ok(())
}

/// 位置由 IR 编辑者维护；非法位置不能静默附着到邻近内容。
pub(super) fn validate(request: &crate::ir::request::Request) -> Result<()> {
    use crate::ir::cache::CacheLocation as L;
    for (index, point) in request.cache_breakpoints.iter().enumerate() {
        let valid = match point.location {
            L::Instruction(i) => i < request.instructions.len(),
            L::Message { message, part } => request
                .messages
                .get(message)
                .is_some_and(|m| part < m.parts.len()),
            L::Tool(i) => i < request.tools.len(),
        };
        if !valid
            || request.cache_breakpoints[..index]
                .iter()
                .any(|other| other.location == point.location)
        {
            return Err(unsupported("cache.breakpoint", "断点位置无效或重复"));
        }
    }
    Ok(())
}

/// Messages 最多四个显式断点，较长 TTL 必须位于较短 TTL 之前。
/// 参考：https://platform.claude.com/docs/en/build-with-claude/prompt-caching
pub(super) fn validate_messages(body: &Request) -> Result<()> {
    use crate::protocol::{
        OptionalNullable as O,
        messages::request::{
            Content, ContentBlock, KnownContentBlock, body::SystemPrompt, tool::Tool,
        },
    };
    let Request::Messages(body) = body else {
        return Ok(());
    };
    let mut ttls = Vec::new();
    for tool in body.tools.as_option().into_iter().flatten() {
        let extra = match tool {
            Tool::Function(f) => &f.extra,
            Tool::Other(fields) => fields,
        };
        if let Some(value) = extra.get("cache_control").filter(|v| !v.is_null()) {
            ttls.push(
                value
                    .get("ttl")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("5m"),
            );
        }
    }
    if let O::Value(SystemPrompt::Parts(parts)) = &body.system {
        for part in parts {
            if let O::Value(control) = &part.cache_control {
                ttls.push(control.ttl.as_option().map(String::as_str).unwrap_or("5m"));
            }
        }
    }
    let mut last_control = None;
    for message in &body.messages {
        if let Content::Parts(parts) = &message.content {
            for part in parts {
                let field = match part {
                    ContentBlock::Known(
                        KnownContentBlock::Text { cache_control, .. }
                        | KnownContentBlock::Image { cache_control, .. }
                        | KnownContentBlock::Document { cache_control, .. }
                        | KnownContentBlock::ToolUse { cache_control, .. }
                        | KnownContentBlock::ToolResult { cache_control, .. },
                    ) => cache_control,
                    _ => continue,
                };
                last_control = field.as_option();
                if let O::Value(control) = field {
                    ttls.push(control.ttl.as_option().map(String::as_str).unwrap_or("5m"));
                }
            }
        } else {
            last_control = None;
        }
    }
    if ttls.len() > 4 {
        return Err(unsupported(
            "cache.breakpoint",
            "Messages 最多支持四个显式断点",
        ));
    }
    if ttls.windows(2).any(|pair| pair == ["5m", "1h"]) {
        return Err(unsupported(
            "cache.breakpoint.ttl",
            "Messages 的 1h 断点必须在 5m 断点之前",
        ));
    }
    if let O::Value(control) = &body.cache_control {
        let automatic_ttl = control.ttl.as_option().map(String::as_str).unwrap_or("5m");
        if let Some(last) = last_control
            && last.ttl.as_option().map(String::as_str).unwrap_or("5m") != automatic_ttl
        {
            return Err(unsupported(
                "cache.ttl",
                "自动缓存与最后内容块的显式 TTL 冲突",
            ));
        }
        if ttls.len() == 4 && last_control.is_none() {
            return Err(unsupported(
                "cache.mode",
                "四个显式断点不能再叠加自动缓存断点",
            ));
        }
        if ttls.contains(&"5m") && control.ttl.as_option().is_some_and(|ttl| ttl == "1h") {
            return Err(unsupported(
                "cache.ttl",
                "自动缓存的 TTL 不能长于此前显式断点",
            ));
        }
    }
    Ok(())
}
