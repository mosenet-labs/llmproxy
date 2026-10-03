//! 从已知缓存叶子提取断点位置，跨协议编码不回读原始报文。
use super::Notes;
use crate::{
    adapter::wire,
    ir::{
        cache::{Breakpoint, CacheLocation},
        request::Request,
    },
    protocol::{OptionalNullable as O, Request as Raw},
};
use serde_json::Value;

/// 记录消息、系统指令及函数声明的缓存前缀边界。
pub(super) fn decode(request: &mut Request, source: &Raw, notes: &mut Notes) {
    for (message, m) in request.messages.iter().enumerate() {
        for (part, p) in m.parts.iter().enumerate() {
            let extra = wire::extra(&p.metadata, request.source_protocol());
            if let Some(value) = extra
                .get("cache_control")
                .or_else(|| extra.get("prompt_cache_breakpoint"))
            {
                add(
                    &mut request.cache_breakpoints,
                    CacheLocation::Message { message, part },
                    value,
                    notes,
                );
            }
        }
    }
    if let Raw::Messages(body) = source {
        if let O::Value(crate::protocol::messages::request::body::SystemPrompt::Parts(parts)) =
            &body.system
        {
            for (index, part) in parts.iter().enumerate() {
                if let O::Value(control) = &part.cache_control {
                    add(
                        &mut request.cache_breakpoints,
                        CacheLocation::Instruction(index),
                        &serde_json::to_value(control).expect("缓存字段可序列化"),
                        notes,
                    );
                }
            }
        }
        let mut index = 0;
        for tool in body.tools.as_option().into_iter().flatten() {
            if let crate::protocol::messages::request::tool::Tool::Function(function) = tool {
                if function
                    .r#type
                    .as_option()
                    .is_some_and(|kind| kind != "custom")
                {
                    continue;
                }
                if let Some(value) = function.extra.get("cache_control") {
                    add(
                        &mut request.cache_breakpoints,
                        CacheLocation::Tool(index),
                        value,
                        notes,
                    );
                }
                index += 1;
            }
        }
    }
}

/// 明确的临时／显式断点才进入 IR；未知模式不擅自解释为缓存。
fn add(output: &mut Vec<Breakpoint>, location: CacheLocation, value: &Value, notes: &mut Notes) {
    if value.is_null() {
        return;
    }
    let mode = value
        .get("type")
        .or_else(|| value.get("mode"))
        .and_then(Value::as_str);
    if !matches!(mode, Some("ephemeral" | "explicit")) {
        notes.reject("cache.breakpoint", "未知缓存断点模式");
        return;
    }
    if value
        .get("ttl")
        .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        notes.reject("cache.breakpoint.ttl", "缓存 TTL 必须是字符串");
        return;
    }
    if let Some(fields) = value.as_object() {
        for key in fields
            .keys()
            .filter(|k| !matches!(k.as_str(), "type" | "mode" | "ttl"))
        {
            notes.dropped(format!("cache.breakpoint.{key}"));
        }
    }
    output.push(Breakpoint {
        location,
        ttl: value.get("ttl").and_then(Value::as_str).map(str::to_owned),
    });
}
