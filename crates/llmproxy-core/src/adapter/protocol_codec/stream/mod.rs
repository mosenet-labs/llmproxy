//! 有状态流式编解码；输入输出为协议结构和事件 IR，SSE 留在接入边界。

mod decode;
mod encode;

pub use decode::Decoder;
pub use encode::Encoder;

#[cfg(test)]
mod tests;
