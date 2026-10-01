//! 多种原始协议共用的固定消息字段值。

use serde::{Deserialize, Serialize};

/// 模型输出消息的固定角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantRole {
    /// 模型回复。
    Assistant,
}

/// 消息对象的固定类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    /// 消息对象。
    Message,
}
