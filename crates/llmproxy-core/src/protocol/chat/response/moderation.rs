//! Chat Completions 响应中的输入与输出审核结果。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// 输入与输出两侧的审核结果；输出数组索引对应候选回复索引。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Moderation {
    /// 输入内容的审核结果或错误。
    pub input: ModerationOutcome,
    /// 生成内容的审核结果或错误。
    pub output: ModerationOutcome,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 单侧审核成功或失败的结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModerationOutcome {
    /// 审核模型返回的分类结果。
    ModerationResults {
        /// 审核模型 ID。
        model: String,
        /// 输入通常只有一项；输出的各项对应生成候选。
        results: Vec<ModerationResult>,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 审核阶段出现的错误。
    Error {
        /// 错误代码。
        code: String,
        /// 错误说明。
        message: String,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 一份内容的审核分类与分数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModerationResult {
    /// 每个审核类别是否命中。
    pub categories: BTreeMap<String, bool>,
    /// 每个类别适用的输入模态，例如文本和图片。
    pub category_applied_input_types: BTreeMap<String, Vec<String>>,
    /// 每个审核类别的分数。
    pub category_scores: BTreeMap<String, f64>,
    /// 是否有任一类别被标记。
    pub flagged: bool,
    /// 产生此结果的审核模型 ID。
    pub model: String,
    /// 结果类型，标准值为 `moderation_result`。
    pub r#type: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
