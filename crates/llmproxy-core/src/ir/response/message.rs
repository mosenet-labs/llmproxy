//! 非流式响应消息的中间表示；外层选择、候选和用量不在这里。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::ir::message::{OpaquePart, Part, PartKind, Role, ToolCall, ToolResult};

/// 一条模型响应消息，内容片段保持原始顺序。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// 响应消息的生成者；未指定角色可用于原样保留 Gemini 内容。
    pub role: Role,
    /// 按输出顺序排列的文本、工具调用及其他内容。
    pub parts: Vec<Part>,
    /// 来源协议的消息级剩余字段和原始形状。
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl From<crate::ir::request::Message> for Message {
    fn from(message: crate::ir::request::Message) -> Self {
        Self {
            role: message.role,
            parts: message.parts,
            metadata: message.metadata,
        }
    }
}

impl From<Message> for crate::ir::request::Message {
    fn from(message: Message) -> Self {
        Self {
            role: message.role,
            parts: message.parts,
            metadata: message.metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, Part, PartKind, Role};
    use serde_json::{Map, json};

    #[test]
    fn response_message_preserves_order() {
        let source = Message {
            role: Role::Assistant,
            parts: vec![
                Part {
                    kind: PartKind::Text("hello".into()),
                    metadata: Map::new(),
                },
                Part {
                    kind: PartKind::Refusal("cannot".into()),
                    metadata: Map::new(),
                },
            ],
            metadata: Map::new(),
        };
        assert_eq!(
            serde_json::from_value::<Message>(serde_json::to_value(&source).unwrap()).unwrap(),
            source
        );
        assert_eq!(
            serde_json::to_value(source.parts[0].clone()).unwrap()["kind"]["type"],
            json!("text")
        );
    }
}
