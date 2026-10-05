//! 响应阶段的中间数据结构。

pub mod body;
pub mod candidate;
pub mod failure;
pub mod item;
pub mod message;
pub(crate) mod source;

pub use body::Response;
pub use candidate::{Candidate, FinishReason, Status};
pub use failure::Failure;
pub use item::Item;
pub use message::*;
