//! 候选回复中各词元的对数概率信息。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::optional_nullable::OptionalNullable;

/// 正文词元和拒绝词元的概率列表。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Logprobs {
    /// 正文中的生成词元及其候选词元。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub content: OptionalNullable<Vec<TokenLogprob>>,
    /// 拒绝说明中的生成词元及其候选词元。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub refusal: OptionalNullable<Vec<TokenLogprob>>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 模型实际生成的一个词元。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenLogprob {
    /// 词元文本。
    pub token: String,
    /// 词元对应的 UTF-8 字节；不存在时为 `null`。
    pub bytes: Option<Vec<u8>>,
    /// 模型生成该词元的对数概率。
    pub logprob: f64,
    /// 同一位置概率最高的其他候选词元。
    pub top_logprobs: Vec<TopLogprob>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 同一词元位置的候选词元及其概率。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopLogprob {
    /// 候选词元文本。
    pub token: String,
    /// 候选词元的 UTF-8 字节；不存在时为 `null`。
    pub bytes: Option<Vec<u8>>,
    /// 此候选词元的对数概率。
    pub logprob: f64,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
