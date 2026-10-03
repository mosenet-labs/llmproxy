//! 解码阶段发现的未映射语义；不保存原始正文。

use serde::{Deserialize, Serialize};

/// 来源语义无法完整进入通用 IR 时的处理依据。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// 来源字段路径。
    pub path: String,
    /// 不包含字段值的原因。
    pub reason: String,
    /// 为真时禁止从通用 IR 编码，仍允许保留来源类型的同协议往返。
    pub reject: bool,
}
