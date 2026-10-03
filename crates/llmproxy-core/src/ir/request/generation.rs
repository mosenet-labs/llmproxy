//! 请求级通用生成参数；未指定值不生成目标字段。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Generation {
    /// 最大输出词元数；Chat 的旧版 max_tokens 暂不归入此字段。
    pub max_output_tokens: Option<u64>,
    /// 采样温度；目标协议范围不兼容时明确拒绝。
    pub temperature: Option<f64>,
    /// 核采样概率上限。
    pub top_p: Option<f64>,
    /// 遇到任一序列时停止；None 与显式空列表区分。
    pub stop_sequences: Option<Vec<String>>,
    /// 请求的候选数量；当前跨协议仅支持单候选。
    pub candidate_count: Option<u64>,
    /// 是否请求事件流；非流式编码器会拒绝 true。
    pub stream: bool,
}
