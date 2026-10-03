//! 客户端函数工具声明；工具调用与结果位于有序消息项中。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function {
    /// 函数名称。
    pub name: String,
    /// 函数用途描述。
    pub description: Option<String>,
    /// 标准 JSON Schema 参数对象；Gemini 原生类型枚举在解码时规范化。
    pub parameters: Value,
    /// 是否要求严格遵循参数 Schema；目标不支持时返回警告。
    pub strict: Option<bool>,
}
