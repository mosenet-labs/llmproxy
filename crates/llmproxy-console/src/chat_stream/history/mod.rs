//! 对话上下文保存 Core IR；展示块、模型统计和协议外壳不作为输入历史。
mod prepare;
mod stream;
#[cfg(test)]
mod tests;
mod tools;
pub(super) use stream::Collector;

use llmproxy_core::{adapter::protocol_codec::Conversion, ir::request::Request};

pub use llmproxy_core::conversation::{Content, Selection};

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
