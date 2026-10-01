//! 仅保存未规范化的原协议字段，避免与 IR 正文重复。

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

pub(super) fn take_string(map: &mut Map<String, Value>, key: &str) -> Result<String> {
    map.remove(key)
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| Error::Invalid(format!("字段 {key} 必须是字符串")))
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
    let mut metadata = Map::new();
    save(&mut metadata, protocol, form, extra);
    Part { kind, metadata }
}

/// 未支持的内容块只保留一份原始对象，供同协议写回。
pub(super) fn opaque(protocol: Protocol, block: Map<String, Value>) -> Part {
    Part {
        kind: PartKind::Opaque(OpaquePart {
            protocol,
            data: Value::Object(block),
        }),
        metadata: Map::new(),
    }
}

/// 三种 `type` 标记协议共用的文本块解码。
pub(super) fn typed_text(
    mut block: Map<String, Value>,
    protocol: Protocol,
    form: &str,
) -> Result<Part> {
    block.remove("type");
    let text = take_string(&mut block, "text")?;
    Ok(text_part(text, protocol, form, block))
}

/// Chat 和 Responses 共用的拒绝块解码。
pub(super) fn typed_refusal(mut block: Map<String, Value>, protocol: Protocol) -> Result<Part> {
    block.remove("type");
    let refusal = take_string(&mut block, "refusal")?;
    Ok(part(PartKind::Refusal(refusal), protocol, "refusal", block))
}

/// IR 字段覆盖来源协议的剩余字段。
pub(super) fn encode_block(part: &Part, protocol: Protocol, normalized: Value) -> Result<Value> {
    Ok(Value::Object(merge(
        extra(&part.metadata, protocol),
        object(normalized)?,
    )))
}

/// 将独立工具结果并入前一条用户消息，供块数组协议共用。
pub(super) fn append_to_previous_user(
    output: &mut [Value],
    content_key: &str,
    blocks: &mut Vec<Value>,
) -> bool {
    if output
        .last()
        .and_then(|v| v.get("role"))
        .and_then(Value::as_str)
        != Some("user")
    {
        return false;
    }
    let Some(Value::Array(previous)) = output.last_mut().and_then(|v| v.get_mut(content_key))
    else {
        return false;
    };
    previous.append(blocks);
    true
}

pub(super) fn message(
    role: Role,
    parts: Vec<Part>,
    protocol: Protocol,
    form: &str,
    extra: Map<String, Value>,
) -> Message {
    let mut metadata = Map::new();
    save(&mut metadata, protocol, form, extra);
    Message {
        role,
        parts,
        metadata,
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

pub(super) fn merge(
    mut base: Map<String, Value>,
    normalized: Map<String, Value>,
) -> Map<String, Value> {
    base.extend(normalized);
    base
}
