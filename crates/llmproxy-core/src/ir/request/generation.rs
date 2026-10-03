//! 请求级通用生成参数；未指定值不生成目标字段。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Generation {
    /// 最大输出词元数；优先读取 Chat 的 max_completion_tokens，兼容旧版 max_tokens。
    pub max_output_tokens: Option<u64>,
    /// 采样温度；目标协议范围不兼容时明确拒绝。
    pub temperature: Option<f64>,
    /// 核采样概率上限。
    pub top_p: Option<f64>,
    /// 遇到任一序列时停止；None 与显式空列表区分。
    pub stop_sequences: Option<Vec<String>>,
    /// 请求的候选数量；目标不支持多个候选时降为一个并警告。
    pub candidate_count: Option<u64>,
    /// 最大采样候选词元数。
    pub top_k: Option<u64>,
    /// 随机种子；只在目标有对应字段时传递。
    pub seed: Option<i64>,
    /// 已出现词元的频率惩罚。
    pub frequency_penalty: Option<f64>,
    /// 已出现话题的存在惩罚。
    pub presence_penalty: Option<f64>,
    /// 是否返回输出词元概率。
    pub logprobs: Option<bool>,
    /// 每个输出词元返回多少个候选概率。
    pub top_logprobs: Option<u64>,
    /// 工具调用的选择约束。
    pub tool_choice: Option<super::controls::ToolChoice>,
    /// 是否允许并行工具调用。
    pub parallel_tool_calls: Option<bool>,
    /// 结构化输出约束。
    pub output_format: Option<super::controls::OutputFormat>,
    /// 推理模式、预算及摘要。
    pub reasoning: super::controls::Reasoning,
    /// 是否请求事件流；非流式编码器会拒绝 true。
    pub stream: bool,
}
