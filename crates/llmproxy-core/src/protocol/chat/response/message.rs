//! `choices[].message` 的原始协议结构，不包含 Choice 外层字段。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::chat::request::message::{FunctionCall, ToolCall};
pub use crate::protocol::common::AssistantRole;
pub use crate::protocol::optional_nullable::OptionalNullable;

/// 非流式 Chat Completion 中模型生成的消息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 模型生成的文本；工具调用或音频输出时可能为 `null`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub content: OptionalNullable<String>,
    /// 模型拒绝回答时给出的说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub refusal: OptionalNullable<String>,
    /// 消息角色，固定为 `assistant`。
    pub role: AssistantRole,
    /// 模型输出中的 URL 引用等标注。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub annotations: OptionalNullable<Vec<Annotation>>,
    /// 模型生成的音频输出。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio: OptionalNullable<Audio>,
    /// 旧版函数调用信息。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_call: OptionalNullable<FunctionCall>,
    /// 模型发起的函数或自定义工具调用。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_calls: OptionalNullable<Vec<ToolCall>>,
    /// 保留未声明的供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 模型输出音频及可在后续请求引用的 ID。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Audio {
    /// 音频输出的唯一 ID。
    pub id: String,
    /// Base64 编码的音频字节。
    pub data: String,
    /// 音频有效期的 Unix 时间戳。
    pub expires_at: i64,
    /// 音频对应的文本转录。
    pub transcript: String,
    /// 保留未声明的音频扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出标注；已知 URL 引用结构化，未来新增类型保留原始 JSON。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Annotation {
    /// 网页搜索生成的 URL 引用。
    UrlCitation(UrlCitationAnnotation),
    /// 尚未建模的标注类型。
    Other(Value),
}

/// URL 引用标注的外层类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UrlCitationAnnotation {
    /// 标注类型，标准值为 `url_citation`。
    pub r#type: String,
    /// 引用在输出中的位置、标题和地址。
    pub url_citation: UrlCitation,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 网页引用在输出文本中的范围与来源。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UrlCitation {
    /// 引用结束位置。
    pub end_index: u64,
    /// 引用起始位置。
    pub start_index: u64,
    /// 来源网页标题。
    pub title: String,
    /// 来源网页地址。
    pub url: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::Message;
    use serde_json::json;

    #[test]
    fn response_message_round_trip() {
        let source = json!({"role":"assistant","content":null,"refusal":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{}"}}],"annotation_extra":true});
        let message: Message = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(message).unwrap(), source);
    }
}
