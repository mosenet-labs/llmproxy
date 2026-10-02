//! 请求阶段的中间数据结构。

pub mod body;
pub mod message;
pub(crate) mod source;

pub use body::Request;
pub use message::*;
