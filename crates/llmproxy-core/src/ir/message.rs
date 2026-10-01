//! 请求与响应消息共用的角色、片段及工具语义。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::Protocol;

/// 跨协议可识别的消息发送方。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// 原协议未指定角色；目前仅 Gemini 可原样写回。
    Unspecified,
    /// 系统指令。
    System,
    /// 开发者指令。
    Developer,
    /// 用户输入。
    User,
    /// 模型回复。
    Assistant,
    /// 独立的工具或旧版函数结果。
    Tool,
}

/// 一段具有独立语义的内容及其附加信息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// 片段的通用语义类型。
    pub kind: PartKind,
    /// 片段级附加信息，例如引用、缓存标记或思考签名。
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// 请求与响应中可归一化的内容类别。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PartKind {
    /// 可直接读取或压缩的文本。
    Text(String),
    /// 图片来源；具体 URL、文件 ID 或内联数据暂以 JSON 保存。
    Image(Value),
    /// 音频来源；具体表示暂以 JSON 保存。
    Audio(Value),
    /// 视频来源；具体表示暂以 JSON 保存。
    Video(Value),
    /// 文件或文档来源；具体表示暂以 JSON 保存。
    File(Value),
    /// 模型发出的工具调用。
    ToolCall(ToolCall),
    /// 客户端返回的工具执行结果。
    ToolResult(ToolResult),
    /// 模型思考内容，包括无法直接读取的加密数据。
    Reasoning(Value),
    /// 模型给出的拒绝说明。
    Refusal(String),
    /// 暂不能归一化的协议片段；跨协议编码时需显式处理。
    Opaque(OpaquePart),
}

/// 通用工具调用信息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// 调用 ID；部分协议或历史消息可能未提供。
    pub id: Option<String>,
    /// 被调用工具的名称。
    pub name: String,
    /// 调用参数；保留 JSON 对象或原协议中的 JSON 文本。
    pub arguments: Value,
}

/// 通用工具结果信息。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// 对应工具调用的 ID；部分协议仅提供名称。
    pub id: Option<String>,
    /// 工具名称；部分协议仅提供调用 ID。
    pub name: Option<String>,
    /// 结果正文；保留文本、内容块数组或结构化 JSON。
    pub content: Value,
}

/// 尚未规范化的原始协议片段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpaquePart {
    /// 原始片段所属的协议，用于避免误写入不兼容的目标协议。
    pub protocol: Protocol,
    /// 原始片段的 JSON 数据。
    pub data: Value,
}
