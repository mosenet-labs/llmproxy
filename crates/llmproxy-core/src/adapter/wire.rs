//! 请求与响应共用的原协议保留字段和内容块操作。

use serde_json::{Map, Value, json};

use crate::{
    ir::request::message::{Message, OpaquePart, Part, PartKind, Role},
    protocol::Protocol,
};

use super::{Error, Result};

const KEY: &str = "_llmproxy_wire";

pub(super) fn save(
    metadata: &mut Map<String, Value>,
    protocol: Protocol,
    form: &str,
    extra: Map<String, Value>,
) {
    metadata.insert(
        KEY.into(),
        json!({"protocol": protocol.as_str(), "form": form, "extra": extra}),
    );
}

fn metadata(protocol: Protocol, form: &str, extra: Map<String, Value>) -> Map<String, Value> {
    let mut metadata = Map::new();
    save(&mut metadata, protocol, form, extra);
    metadata
}

pub(super) fn form(metadata: &Map<String, Value>, protocol: Protocol) -> Option<&str> {
    let wire = metadata.get(KEY)?;
    if wire.get("protocol")?.as_str()? != protocol.as_str() {
        return None;
    }
    wire.get("form")?.as_str()
}

pub(super) fn extra(metadata: &Map<String, Value>, protocol: Protocol) -> Map<String, Value> {
    if form(metadata, protocol).is_none() {
        return Map::new();
    }
    metadata
        .get(KEY)
        .and_then(|v| v.get("extra"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn object(value: Value) -> Result<Map<String, Value>> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(Error::Invalid("消息或内容块必须是 JSON 对象".into())),
    }
}

pub(super) fn text_part(
    text: String,
    protocol: Protocol,
    form: &str,
    extra: Map<String, Value>,
) -> Part {
    part(PartKind::Text(text), protocol, form, extra)
}

/// 为已规范化的片段附上来源协议和剩余字段。
pub(super) fn part(
    kind: PartKind,
    protocol: Protocol,
    form: &str,
    extra: Map<String, Value>,
) -> Part {
    Part {
        kind,
        metadata: metadata(protocol, form, extra),
    }
}

/// 未支持的内容块只保留一份原始对象，供同协议写回。
pub(super) fn opaque(protocol: Protocol, block: Map<String, Value>) -> Part {
    if let Some(output) = super::server_output::decode(protocol, &block) {
        return Part {
            kind: PartKind::ServerOutput(output),
            metadata: Map::new(),
        };
    }
    Part {
        kind: PartKind::Opaque(OpaquePart {
            protocol,
            data: Value::Object(block),
        }),
        metadata: Map::new(),
    }
}

pub(super) fn message(
    role: Role,
    parts: Vec<Part>,
    protocol: Protocol,
    form: &str,
    extra: Map<String, Value>,
) -> Message {
    Message {
        role,
        parts,
        metadata: metadata(protocol, form, extra),
    }
}

pub(super) fn response_message(
    role: Role,
    parts: Vec<Part>,
    protocol: Protocol,
    form: &str,
    extra: Map<String, Value>,
) -> crate::ir::response::Message {
    crate::ir::response::Message {
        role,
        parts,
        metadata: metadata(protocol, form, extra),
    }
}

pub(super) fn unsupported_role(role: Role) -> Error {
    Error::Unsupported(format!("目标协议不能表示角色 {role:?}"))
}

pub(super) fn reject_unmapped_source(message: &Message, target: Protocol) -> Result<()> {
    if target == Protocol::OpenAiChat {
        return Ok(());
    }
    let extra = extra(&message.metadata, Protocol::OpenAiChat);
    for key in ["audio", "function_call", "refusal"] {
        if extra.get(key).is_some_and(|value| !value.is_null()) {
            return Err(Error::Unsupported(format!(
                "Chat 字段 {key} 尚不能跨协议转换"
            )));
        }
    }
    Ok(())
}

/// 将尚未规范化的可选叶子字段放入 IR 元数据，保持缺失和 null。
pub(super) fn put<T: serde::Serialize>(
    extra: &mut Map<String, Value>,
    key: &str,
    value: &crate::protocol::OptionalNullable<T>,
) -> Result<()> {
    if !value.is_missing() {
        extra.insert(key.into(), serde_json::to_value(value)?);
    }
    Ok(())
}
/// 保留协议中普通 Option 字段的有值形态。
pub(super) fn put_option<T: serde::Serialize>(
    extra: &mut Map<String, Value>,
    key: &str,
    value: &Option<T>,
) -> Result<()> {
    if let Some(value) = value {
        extra.insert(key.into(), serde_json::to_value(value)?);
    }
    Ok(())
}
/// 从 IR 元数据恢复一个可选叶子字段；不反序列化完整消息。
pub(super) fn take<T: serde::de::DeserializeOwned>(
    extra: &mut Map<String, Value>,
    key: &str,
) -> Result<crate::protocol::OptionalNullable<T>> {
    match extra.remove(key) {
        None => Ok(crate::protocol::OptionalNullable::Missing),
        Some(value) => Ok(serde_json::from_value(value)?),
    }
}
/// 从元数据恢复普通 Option 字段。
pub(super) fn take_option<T: serde::de::DeserializeOwned>(
    extra: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<T>> {
    extra
        .remove(key)
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}
/// 必须由来源元数据提供的响应叶子字段。
pub(super) fn required<T: serde::de::DeserializeOwned>(
    extra: &mut Map<String, Value>,
    key: &str,
) -> Result<T> {
    serde_json::from_value(
        extra
            .remove(key)
            .ok_or_else(|| Error::Unsupported(format!("缺少来源字段 {key}")))?,
    )
    .map_err(Into::into)
}
/// 尚未规范化的内容块保留为不透明叶子，不能用作整包协议正文。
pub(super) fn opaque_value<T: serde::Serialize>(protocol: Protocol, value: &T) -> Result<Part> {
    Ok(opaque(protocol, object(serde_json::to_value(value)?)?))
}
