//! 可持久化的有序对话内容，复用协议无关的 IR 类型。
use crate::{
    ir::{
        message::{Part, PartKind, Role},
        request::{Item, Message},
        response,
    },
    protocol::Protocol,
};
/// 当前选择与每轮来源使用同一标识，模型／路由 ID 不属于提示词。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Selection {
    /// 模型 ID 或带 route: 前缀的路由 ID。
    pub model_id: String,
    /// 本轮客户端入口协议。
    pub protocol: Protocol,
}

/// 一次输入或回复的有序 IR 内容，复用 Core 的消息和输入项类型。
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Content {
    /// 保存完整角色与片段，不从展示文案重建。
    pub messages: Vec<Message>,
    /// 消息索引与独立工具项的原有顺序。
    pub items: Vec<Item>,
}
impl Content {
    /// 用户输入与已完成的文本回复共用标准消息构造。
    pub fn text(role: Role, text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        Self {
            messages: vec![Message {
                role,
                parts: vec![Part {
                    kind: PartKind::Text(text.into()),
                    metadata: Default::default(),
                }],
                metadata: Default::default(),
            }],
            items: vec![Item::Message(0)],
        }
    }

    /// 与页面选择相同的最低序号候选，独立工具输出也保持原有位置。
    pub fn response(response: &response::Response) -> Self {
        let positions = response
            .candidates
            .iter()
            .min_by_key(|candidate| candidate.index)
            .map(|candidate| candidate.items.clone())
            .unwrap_or_else(|| (0..response.items.len()).collect());
        let mut content = Self::default();
        for position in positions {
            let Some(item) = response.items.get(position) else {
                continue;
            };
            let item = match item {
                response::Item::Message(index) => {
                    let Some(message) = response.messages.get(*index) else {
                        continue;
                    };
                    let index = content.messages.len();
                    content.messages.push(message.clone().into());
                    Item::Message(index)
                }
                response::Item::ToolCall { call, item_id } => Item::ToolCall {
                    call: call.clone(),
                    item_id: item_id.clone(),
                },
                response::Item::Reasoning(text) => Item::Reasoning(text.clone()),
                response::Item::ServerOutput(output) => Item::ServerOutput(output.clone()),
                response::Item::Opaque(value) => Item::Opaque(value.clone()),
            };
            content.items.push(item);
        }
        content
    }
}
