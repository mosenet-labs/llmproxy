//! 请求消息的中间表示；此模块只定义数据，不执行协议解析或编码。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::Protocol;

/// 一轮请求消息，保留内容片段的原有顺序。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 消息发送方；工具结果也可以作为独立消息出现。
    pub role: Role,
    /// 依次排列的文本、多模态或工具内容。
    pub parts: Vec<Part>,
    /// 消息级附加信息，例如原协议的名称、阶段或状态。
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// 跨协议可识别的消息发送方。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
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

/// 四种请求协议中可归一化的内容类别。
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

#[cfg(test)]
mod tests {
    use super::{Message, OpaquePart, Part, PartKind, Role, ToolCall};
    use crate::protocol::Protocol;
    use serde_json::{Map, json};

    #[test]
    fn mixed_message_keeps_part_order_and_opaque_origin() {
        let message = Message {
            role: Role::Assistant,
            parts: vec![
                Part {
                    kind: PartKind::Text("I'll check".into()),
                    metadata: Map::new(),
                },
                Part {
                    kind: PartKind::ToolCall(ToolCall {
                        id: Some("call_1".into()),
                        name: "lookup".into(),
                        arguments: json!({"query": "x"}),
                    }),
                    metadata: Map::new(),
                },
                Part {
                    kind: PartKind::Opaque(OpaquePart {
                        protocol: Protocol::Gemini,
                        data: json!({"toolCall": {"toolType": "GOOGLE_SEARCH_WEB"}}),
                    }),
                    metadata: Map::new(),
                },
            ],
            metadata: Map::new(),
        };

        let encoded = serde_json::to_value(&message).unwrap();
        let decoded: Message = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, message);
        assert!(matches!(decoded.parts[0].kind, PartKind::Text(_)));
        assert!(matches!(decoded.parts[1].kind, PartKind::ToolCall(_)));
        assert!(matches!(decoded.parts[2].kind, PartKind::Opaque(_)));
    }
}
