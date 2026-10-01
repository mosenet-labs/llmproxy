//! 与具体供应商协议无关的 token 用量表示。

use serde::{Deserialize, Serialize};

use super::cache::CacheUsage;

/// 一次模型响应的用量；所有计数以供应商实际报告为准。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入词元数。
    pub input_tokens: Option<u64>,
    /// 输出词元数。
    pub output_tokens: Option<u64>,
    /// 输入与输出合计词元数。
    pub total_tokens: Option<u64>,
    /// 输入缓存的读取与写入词元数。
    pub cache: CacheUsage,
    /// 输入模态的细分词元数。
    pub input_details: InputTokenDetails,
    /// 输出模态、推理和预测词元数。
    pub output_details: OutputTokenDetails,
}

/// 输入模态细分；其值可能与缓存计数重叠，不应直接相加。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputTokenDetails {
    /// 输入文本词元数。
    pub text_tokens: Option<u64>,
    /// 输入音频词元数。
    pub audio_tokens: Option<u64>,
    /// 输入图片词元数。
    pub image_tokens: Option<u64>,
}

/// 输出模态和生成方式的细分；其值可能相互重叠。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputTokenDetails {
    /// 输出文本词元数。
    pub text_tokens: Option<u64>,
    /// 输出音频词元数。
    pub audio_tokens: Option<u64>,
    /// 推理词元数。
    pub reasoning_tokens: Option<u64>,
    /// 与预测输出匹配的词元数。
    pub accepted_prediction_tokens: Option<u64>,
    /// 与预测输出未匹配的词元数。
    pub rejected_prediction_tokens: Option<u64>,
}
