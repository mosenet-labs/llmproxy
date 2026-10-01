//! Chat Completions 请求中的原始数据结构。

pub mod body;
pub mod message;
pub mod parameters;

pub use body::Request;
pub use message::*;
pub use parameters::*;
