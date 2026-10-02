//! Gemini 候选 Content 与响应 IR。

mod decode;
mod encode;
mod usage;

pub use usage::{decode_gemini_usage, encode_gemini_usage};

pub use decode::decode_gemini;
pub use encode::encode_gemini;
