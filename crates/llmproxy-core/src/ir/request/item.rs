//! 请求中按原顺序排列的消息和 Responses 独立输入项。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ir::message::{Role, ToolCall, ToolResult};

/// 顶层指令的权限来源；跨协议降级时需要保留该区别。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Instruction {
    /// 原始指令的角色；Responses 顶层 instructions 按系统指令处理。
    pub role: Role,
    /// 指令文本。
    pub text: String,
}

/// 输入项索引指向 `Request.messages`，避免复制消息和编辑后产生分歧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Item {
    /// 一条普通消息在 `Request.messages` 中的位置。
    Message(usize),
    /// Responses 独立函数调用；`item_id` 是输出项 ID，`call.id` 是配对 ID。
    ToolCall {
        call: ToolCall,
        item_id: Option<String>,
    },
    /// Responses 独立函数结果。
    ToolResult(ToolResult),
    /// 尚无跨协议语义的独立输入项。
    Opaque(Value),
}
