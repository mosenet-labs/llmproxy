//! Gemini `generateContent` 请求中 `contents` 数组的原始协议结构。
//! 参考 API：https://ai.google.dev/api/generate-content
//! `Content`、`Part` 及其子对象的字段顺序遵循上述 API 文档。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::optional_nullable::OptionalNullable;

/// `contents` 数组中的一轮消息，对应 API 的 `Content`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 按顺序组成此轮消息的内容片段。
    pub parts: Vec<Part>,
    /// 消息发送方；省略时由 API 根据上下文判断。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `Content.role` 允许的发送方。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 用户消息或客户端返回的函数结果。
    User,
    /// 模型生成的消息或函数调用。
    Model,
}

/// 一段文本、媒体或工具数据，对应 API 的 `Part`。
/// API 将正文各字段视为互斥联合；此原始载体保留字段，暂不验证互斥关系。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    /// 此片段是否为模型的思考内容。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thought: OptionalNullable<bool>,
    /// 回传模型思考或工具调用时需要保留的不透明签名。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thought_signature: OptionalNullable<String>,
    /// 片段的自定义元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub part_metadata: OptionalNullable<Map<String, Value>>,
    /// 输入媒体的分辨率配置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub media_resolution: OptionalNullable<Value>,
    /// 视频媒体的处理方式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub media_processing: OptionalNullable<String>,
    /// 音频转录数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub audio_transcription: OptionalNullable<Value>,
    /// 文本对应的语音合成元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub speech_metadata: OptionalNullable<Value>,
    /// 内联文本。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub text: OptionalNullable<String>,
    /// 直接嵌入的媒体字节及 MIME 类型。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub inline_data: OptionalNullable<Blob>,
    /// 模型预测的函数调用。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_call: OptionalNullable<FunctionCall>,
    /// 客户端返回的函数执行结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub function_response: OptionalNullable<FunctionResponse>,
    /// 通过 URI 引用的媒体或文件。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub file_data: OptionalNullable<FileData>,
    /// 模型生成的可执行代码；具体结构暂以 JSON 保留。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub executable_code: OptionalNullable<Value>,
    /// 代码执行结果；具体结构暂以 JSON 保留。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub code_execution_result: OptionalNullable<Value>,
    /// 服务端工具调用；具体结构暂以 JSON 保留。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_call: OptionalNullable<Value>,
    /// 服务端工具结果；具体结构暂以 JSON 保留。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub tool_response: OptionalNullable<Value>,
    /// 视频片段的时间戳等元数据。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub video_metadata: OptionalNullable<Value>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `inlineData` 中的媒体字节。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    /// 媒体的 IANA MIME 类型。
    pub mime_type: String,
    /// Base64 编码的媒体字节。
    pub data: String,
    /// 供模型引用此媒体的可选名称。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub display_name: OptionalNullable<String>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `functionCall` 中模型预测的函数调用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// 可用于匹配函数结果的调用 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 要调用的函数名。
    pub name: String,
    /// 传给函数的 JSON 参数对象。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub args: OptionalNullable<Map<String, Value>>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `functionResponse` 中客户端返回的函数结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionResponse {
    /// 对应函数调用的 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 已执行函数的名称。
    pub name: String,
    /// 函数返回的 JSON 对象。
    pub response: Map<String, Value>,
    /// 函数返回的多模态内容片段；具体结构暂以 JSON 保留。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parts: OptionalNullable<Vec<Value>>,
    /// 此函数调用后是否还有后续结果。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub will_continue: OptionalNullable<bool>,
    /// 非阻塞函数结果的调度方式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub scheduling: OptionalNullable<String>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `fileData` 中通过 URI 引用的媒体或文件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    /// 文件的 IANA MIME 类型。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub mime_type: OptionalNullable<String>,
    /// 文件的 URI。
    pub file_uri: String,
    /// 供模型引用此文件的可选名称。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub display_name: OptionalNullable<String>,
    /// 保留当前未声明的扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::{Message, Role};
    use serde_json::json;

    #[test]
    fn request_contents_round_trip() {
        let source = json!([
            {"role": "user", "parts": [
                {"text": "Describe these", "partMetadata": {"source": "client"}},
                {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}},
                {"fileData": {"mimeType": "application/pdf", "fileUri": "files/report"}}
            ]},
            {"role": "model", "parts": [
                {"functionCall": {"id": "call_1", "name": "lookup", "args": {"q": "x"}}, "thoughtSignature": "signature"},
                {"text": "Thinking", "thought": true},
                {"toolCall": {"toolType": "GOOGLE_SEARCH_WEB", "args": {}}, "extension": 1}
            ]},
            {"role": "user", "parts": [
                {"functionResponse": {"id": "call_1", "name": "lookup", "response": {"result": "ok"}}},
                {"text": null}
            ], "providerExtension": true},
            {"parts": [{"text": "Role is optional"}]}
        ]);
        let messages: Vec<Message> = serde_json::from_value(source.clone()).unwrap();

        assert_eq!(messages[0].role, Some(Role::User));
        assert_eq!(messages[1].role, Some(Role::Model));
        assert_eq!(messages[3].role, None);
        assert_eq!(serde_json::to_value(messages).unwrap(), source);
    }

    #[test]
    fn missing_required_fields_are_rejected() {
        assert!(serde_json::from_value::<Message>(json!({"role": "user"})).is_err());
        assert!(
            serde_json::from_value::<Message>(json!({
                "parts": [{"inlineData": {"mimeType": "image/png"}}]
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Message>(json!({
                "parts": [{"functionResponse": {"name": "lookup"}}]
            }))
            .is_err()
        );
    }
}
