//! 与供应商无关的工具选择和结构化输出约束。
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 工具选择模式；具名选择保留允许列表，不能静默放宽为任意工具。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToolChoice {
    /// 由模型决定是否调用。
    Auto,
    /// 禁止调用工具。
    None,
    /// 至少调用一个工具。
    Required,
    /// 仅调用指定名称的工具。
    Named(String),
    /// 将可调用函数限制在名称集合内。
    Allowed {
        /// 是否必须调用集合中的函数。
        required: bool,
        /// 可调用函数名称。
        names: Vec<String>,
    },
}

/// 结构化输出的语义；Schema 保持开放 JSON 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OutputFormat {
    /// 普通文本输出。
    Text,
    /// 合法 JSON 对象，不附加字段约束。
    JsonObject,
    /// 按给定 Schema 约束输出。
    JsonSchema {
        /// 目标需要名称且来源没有名称时使用固定的 response 名称。
        name: Option<String>,
        /// 输出用途说明。
        description: Option<String>,
        /// 开放的 JSON Schema 对象。
        schema: Map<String, Value>,
        /// 是否启用来源协议的严格 Schema 模式。
        strict: Option<bool>,
    },
}

/// 模型推理配置；努力等级和预算分别保存，不猜测两者换算关系。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reasoning {
    /// enabled、disabled 或 adaptive 模式。
    pub mode: Option<String>,
    /// low、medium、high 等级，由目标模型进一步校验。
    pub effort: Option<String>,
    /// 思考 token 预算，-1 表示 Gemini 自动预算。
    pub budget: Option<i64>,
    /// 是否请求可见思考。
    pub include: Option<bool>,
    /// 摘要详细程度。
    pub summary: Option<String>,
}
