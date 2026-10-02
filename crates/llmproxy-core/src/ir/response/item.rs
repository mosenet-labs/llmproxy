//! 非流式响应中按原顺序排列的消息和独立输出项。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ir::message::ToolCall;

/// 输出项索引指向 `Response.messages`，其余项保持原顺序。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Item {
    /// 一条响应消息在 `Response.messages` 中的位置。
    Message(usize),
    /// Responses 独立函数调用；输出项 ID 与调用配对 ID 分开保存。
    ToolCall {
        call: ToolCall,
        item_id: Option<String>,
    },
    /// 推理或服务端工具等尚无跨协议语义的输出项。
    Opaque(Value),
}
