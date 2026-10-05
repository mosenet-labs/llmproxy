//! 从协议类型直接提取整体 IR；不序列化来源正文。
mod cache;
mod controls;
mod native;
mod request;
mod response;
mod tools;
use crate::{ir::diagnostic::Diagnostic, protocol::OptionalNullable};
pub(super) use request::decode as decode_request;
pub(super) use response::decode as decode_response;
pub(super) use response::finish;
pub(super) use response::{chat_usage, gemini_usage, messages_usage, responses_fields};
use serde_json::{Map, Value};

/// 只收集字段路径，未知字段值不会进入转换日志。
#[derive(Default)]
pub(super) struct Notes(Vec<Diagnostic>);
impl Notes {
    /// 交给流式及非流式投影共用，不携带协议正文。
    pub(super) fn into_vec(self) -> Vec<Diagnostic> {
        self.0
    }
    /// 标记一个无法跨协议保留的字段。
    pub(super) fn dropped(&mut self, path: impl Into<String>) {
        self.0.push(Diagnostic {
            path: path.into(),
            reason: "字段尚无目标协议映射，已丢弃".into(),
            reject: false,
        });
    }
    /// 显式 null 也作为来源字段记录。
    pub(super) fn field<T>(&mut self, field: &OptionalNullable<T>, path: &str) {
        if !field.is_missing() {
            self.dropped(path);
        }
    }
    /// 未知扩展只遍历键，不解释字段值。
    pub(super) fn extra(&mut self, extra: &Map<String, Value>, path: &str) {
        for key in extra.keys() {
            self.dropped(format!("{path}.{key}"));
        }
    }
    /// 开放对象仅按已映射键筛选，局部扩展仍逐字段报告。
    pub(super) fn object_extra(&mut self, object: &Map<String, Value>, known: &[&str], path: &str) {
        for key in object.keys().filter(|key| !known.contains(&key.as_str())) {
            self.dropped(format!("{path}.{key}"));
        }
    }
    /// 同协议仍可保留原文，规范化目标编码时明确报错。
    pub(super) fn reject(&mut self, path: &str, reason: &str) {
        self.0.push(Diagnostic {
            path: path.into(),
            reason: reason.into(),
            reject: true,
        });
    }
}
