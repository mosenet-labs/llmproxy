//! Chat Completions 协议的原始数据结构。

pub mod request;
pub mod response;

pub use request::{Request, message::*};
pub use response::{Chunk, Completion};
