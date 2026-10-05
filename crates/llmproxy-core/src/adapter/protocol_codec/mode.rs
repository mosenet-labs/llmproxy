//! 编码意图由调用方明确选择，来源副本的存在不再隐式切换行为。

/// 整体请求或响应的编码方式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeMode {
    /// 同协议回写 IR 的编辑，保留来源字段和原有外壳；要求存在来源副本。
    Preserve,
    /// 从 IR 和目标外壳构造正文，返回无法表达的字段警告；可改变消息数量。
    Rebuild,
}
