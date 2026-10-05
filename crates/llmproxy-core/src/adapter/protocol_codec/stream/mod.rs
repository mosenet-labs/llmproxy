//! 有状态流式编解码；输入输出为协议结构和事件 IR，SSE 留在接入边界。

mod decode;

pub use decode::Decoder;

#[cfg(test)]
mod tests;
