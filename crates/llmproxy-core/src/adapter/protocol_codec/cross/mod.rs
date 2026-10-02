//! 四协议非流式跨协议转换及结构化警告。

mod request;
mod response;

pub(super) use request::encode_request;
pub(super) use response::encode_response;

use crate::{
    adapter::{Error, Result, wire},
    protocol::{Protocol, chat, gemini, messages, responses},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

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

/// 转换后的正文和需要由接入方记录的语义损失。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversion {
    /// 目标协议的 JSON 正文。
    pub body: Value,
    /// 不包含提示词或工具结果正文的警告。
    pub warnings: Vec<ConversionWarning>,
}

impl Conversion {
    /// 同协议无损回写。
    pub fn exact(body: Value) -> Self {
        Self {
            body,
            warnings: Vec::new(),
        }
    }
}

/// 一条可供 Gateway 结构化日志使用的转换警告。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConversionWarning {
    /// 来源协议。
    pub source: Protocol,
    /// 目标协议。
    pub target: Protocol,
    /// 发生降级或丢弃的字段路径。
    pub path: String,
    /// 不含正文内容的原因。
    pub reason: String,
}

/// 只保存字段路径与原因，避免在日志中泄露对话内容。
fn warn(
    warnings: &mut Vec<ConversionWarning>,
    source: Protocol,
    target: Protocol,
    path: &str,
    reason: &str,
) {
    warnings.push(ConversionWarning {
        source,
        target,
        path: path.into(),
        reason: reason.into(),
    });
}

fn warn_keys(
    value: &Value,
    allowed: &[&str],
    path: &str,
    source: Protocol,
    target: Protocol,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported(path, "必须是 JSON 对象"))?;
    for key in object.keys().filter(|key| !allowed.contains(&key.as_str())) {
        warn(
            warnings,
            source,
            target,
            &format!("{path}.{key}"),
            "字段尚无目标协议映射，已丢弃",
        );
    }
    Ok(())
}

fn warn_metadata(
    metadata: &Map<String, Value>,
    source: Protocol,
    target: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    if metadata.contains_key("_llmproxy_wire") && wire::form(metadata, source).is_none() {
        return Err(unsupported(path, "消息来源协议标记不匹配"));
    }
    let mut extra = wire::extra(metadata, source);
    for key in [
        "content",
        "annotations",
        "type",
        "status",
        "id",
        "tool_calls",
    ] {
        if extra.get(key).is_some_and(Value::is_null)
            || key == "annotations" && extra.get(key) == Some(&json!([]))
        {
            extra.remove(key);
        }
    }
    for key in extra.keys().chain(
        metadata
            .keys()
            .filter(|key| key.as_str() != "_llmproxy_wire"),
    ) {
        warn(
            warnings,
            source,
            target,
            &format!("{path}.{key}"),
            "附加字段尚无目标协议映射，已丢弃",
        );
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
