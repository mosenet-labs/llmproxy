//! 服务端工具输出，不能被解释为等待客户端执行的函数调用。
use super::message::OpaquePart;
use serde::{Deserialize, Serialize};

/// 已完成的可见内容与来源专属执行记录分开。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerOutput {
    /// 输出类别。
    pub kind: Kind,
    /// 可供跨协议展示或放入历史的正文；执行动作本身可以没有正文。
    pub text: Option<String>,
    /// 代码语言；只用于代码展示。
    pub language: Option<String>,
    /// 运行结果状态；例如 Gemini 的 OUTCOME_OK。
    pub outcome: Option<String>,
    /// 同协议逐字段往返使用；跨协议编码不得读取。
    pub original: Option<OpaquePart>,
}
/// 不同 Provider 的执行事件归一化类别。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    /// 展示模型在服务端运行的代码。
    Code,
    /// 代码执行的可见日志与结果。
    ExecutionResult,
    /// 搜索结果的公开标题与 URL。
    SearchResult,
    /// 服务端执行动作；不能伪造成客户端工具调用。
    Action,
}
