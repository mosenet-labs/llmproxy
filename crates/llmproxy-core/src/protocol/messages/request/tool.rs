//! Messages 客户端工具声明。
//! 参考：https://platform.claude.com/docs/en/api/messages/create
use crate::protocol::OptionalNullable;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
/// 客户端函数或尚未规范化的服务端工具。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tool {
    Function(Function),
    Other(Map<String, Value>),
}
/// 客户端函数的原始声明。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function {
    /// 参数 JSON Schema。
    pub input_schema: Value,
    /// 工具名称。
    pub name: String,
    /// 工具说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 客户端工具可省略类型，显式类型为 custom。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub r#type: OptionalNullable<String>,
    /// 其余工具参数原样保留。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
