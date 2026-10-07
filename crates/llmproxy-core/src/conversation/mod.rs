//! 对话持久化共用类型；运行状态和数据库操作由上层负责。
mod content;
mod reply;
pub use content::{Content, Selection};
pub use reply::{DisplayMedia, DisplayPart, Reply};
