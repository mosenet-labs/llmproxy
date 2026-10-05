//! 流式响应的事件中间表示；候选、内容块和传输结束分别处理。
mod error;
mod event;
mod state;
mod usage;

pub use error::Error;
pub use event::{Event, Head, Key, Metadata, ToolHead};
pub use state::{Limits, PartState, State};
pub use usage::UsageState;

#[cfg(test)]
mod tests;
