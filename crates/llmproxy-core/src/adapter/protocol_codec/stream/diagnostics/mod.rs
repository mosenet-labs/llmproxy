//! 转换诊断与增量语义分离；只读取类型字段和开放对象的键。
mod chat;
mod gemini;
mod messages;
mod responses;

use crate::{
    adapter::protocol_codec::projection::Notes, ir::diagnostic::Diagnostic, protocol::stream::Event,
};

/// 每个来源事件独立生成诊断，不累计扩展值或跨分片去重集合。
pub(super) fn collect(event: &Event) -> Vec<Diagnostic> {
    let mut notes = Notes::default();
    match event {
        Event::Chat(body) => chat::collect(body, &mut notes),
        Event::Responses(body) => responses::collect(body, &mut notes),
        Event::Messages(body) => messages::collect(body, &mut notes),
        Event::Gemini(body) => gemini::collect(body, &mut notes),
        Event::End(_) => {}
    }
    notes.into_vec()
}
