//! 请求消息的中间表示；此模块只定义数据，不执行协议解析或编码。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::ir::message::{OpaquePart, Part, PartKind, Role, ToolCall, ToolResult};

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
