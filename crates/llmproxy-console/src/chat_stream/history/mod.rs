//! 对话上下文保存 Core IR；展示块、模型统计和协议外壳不作为输入历史。
mod prepare;
mod stream;
#[cfg(test)]
mod tests;
mod tools;
pub(super) use stream::Collector;

use llmproxy_core::{
    adapter::protocol_codec::Conversion,
    ir::{
        message::{Part, PartKind, Role},
        request::{Item, Message, Request},
        response,
    },
    protocol::Protocol,
};

/// 当前选择与每轮来源使用同一标识，模型／路由 ID 不属于提示词。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub model_id: String,
    pub protocol: Protocol,
}

/// 一次输入或回复的有序 IR 内容，复用 Core 的消息和输入项类型。
#[derive(Clone, Debug, Default)]
pub(crate) struct Content {
    pub messages: Vec<Message>,
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

#[derive(Clone, Debug)]
struct Record {
    selection: Selection,
    content: Content,
}

/// 按轮保存内容来源；切换仅改变下一轮目标，不修改之前的 IR。
#[derive(Clone, Debug, Default)]
pub(crate) struct Conversation {
    records: Vec<Record>,
}
impl Conversation {
    /// 成功回复或用户输入提交后才进入可发送的历史。
    pub fn append(&mut self, selection: &Selection, content: Content) {
        if !content.items.is_empty() {
            self.records.push(Record {
                selection: selection.clone(),
                content,
            });
        }
    }

    /// 每轮从保存的历史构造独立发送副本，损失提示不改变原内容。
    pub fn request(&self, target: &Selection) -> Conversion<Request> {
        prepare::request(&self.records, target)
    }
}
