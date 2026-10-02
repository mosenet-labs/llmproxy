//! 整体请求的中间表示；未规范化参数保存在来源协议类型中。

use serde::{Deserialize, Serialize};

use super::{Instruction, Item, Message, source::Source};
use crate::{ir::cache::CacheSettings, protocol::Protocol};

/// 一次完整请求的消息和缓存投影，以及可同协议回写的来源类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 按原报文顺序排列的对话消息。
    pub messages: Vec<Message>,
    /// 在消息历史之前生效的顶层指令。
    pub instructions: Vec<Instruction>,
    /// 消息及独立工具输入项的原始顺序。
    pub items: Vec<Item>,
    /// 请求级缓存设置；内容块级断点仍由消息保留。
    pub cache: CacheSettings,
    /// 未规范化的生成参数、工具输入项及供应商扩展字段。
    pub(crate) source: Source,
}

impl Request {
    /// 返回原报文使用的协议。
    pub fn source_protocol(&self) -> Protocol {
        self.source.protocol()
    }
}
