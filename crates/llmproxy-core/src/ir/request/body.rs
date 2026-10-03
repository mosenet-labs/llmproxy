//! 整体请求的中间表示；未规范化参数保存在来源协议类型中。

use serde::{Deserialize, Serialize};

use super::{Function, Generation, Instruction, Item, Message, source::Source};
use crate::{ir::cache::CacheSettings, protocol::Protocol};

/// 一次完整请求的消息和缓存投影，以及可同协议回写的来源类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 请求中的模型标识；Gemini 的 URL 模型由 Gateway 补入。
    pub model: Option<String>,
    /// 客户端函数工具声明。
    pub tools: Vec<Function>,
    /// 已规范化的生成参数。
    pub generation: Generation,
    /// 尚未进入通用语义的字段及跨协议处理规则。
    pub diagnostics: Vec<crate::ir::diagnostic::Diagnostic>,
    /// 按原报文顺序排列的对话消息。
    pub messages: Vec<Message>,
    /// 在消息历史之前生效的顶层指令。
    pub instructions: Vec<Instruction>,
    /// 消息及独立工具输入项的原始顺序。
    pub items: Vec<Item>,
    /// 请求级缓存设置；内容块级断点仍由消息保留。
    pub cache: CacheSettings,
    /// 未规范化的生成参数、工具输入项及供应商扩展字段。
    pub(crate) source: Option<Source>,
    /// 来源标识仅用于诊断，不要求存在来源报文。
    pub(crate) origin: Protocol,
}

impl Request {
    /// 创建可直接填充并编码到任意协议的 IR，不依赖来源报文。
    pub fn new(origin: Protocol) -> Self {
        Self {
            model: None,
            tools: Vec::new(),
            generation: Generation::default(),
            diagnostics: Vec::new(),
            messages: Vec::new(),
            instructions: Vec::new(),
            items: Vec::new(),
            cache: CacheSettings::default(),
            source: None,
            origin,
        }
    }

    /// 移除同协议往返副本，后续编码完全使用通用字段。
    pub fn without_source(mut self) -> Self {
        self.source = None;
        self
    }

    /// 返回原报文使用的协议。
    pub fn source_protocol(&self) -> Protocol {
        self.origin
    }
}
