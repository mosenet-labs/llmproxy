//! Responses 客户端函数声明、调用和结果，供请求与响应共同使用。
//! 参考：https://developers.openai.com/api/docs/guides/function-calling
use crate::protocol::OptionalNullable;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 函数声明；其他工具保留为扩展类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tool {
    Function(Function),
    Other(Map<String, Value>),
}

/// 客户端函数的协议声明。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function {
    /// 函数名称。
    pub name: String,
    /// 参数 JSON Schema。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parameters: OptionalNullable<Value>,
    /// 是否启用严格校验。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub strict: OptionalNullable<bool>,
    /// 固定为 function。
    pub r#type: FunctionType,
    /// 函数用途。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 未知协议扩展。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
/// 函数声明类型标记。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FunctionType {
    #[serde(rename = "function")]
    Function,
}
/// 函数调用类型标记。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CallType {
    #[serde(rename = "function_call")]
    FunctionCall,
}
/// 函数结果类型标记。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ResultType {
    #[serde(rename = "function_call_output")]
    FunctionCallOutput,
}

/// 独立函数调用；调用 ID 和输出项 ID 分开保存。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// JSON 编码的函数参数。
    pub arguments: String,
    /// 用于匹配结果的调用 ID；兼容 Provider 缺失 ID，转换时显式补号。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub call_id: OptionalNullable<String>,
    /// 函数名称。
    pub name: String,
    /// 固定为 function_call。
    pub r#type: CallType,
    /// 输出项 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 工具项处理状态。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub status: OptionalNullable<String>,
    /// 未知协议扩展。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 客户端提交的函数结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallOutput {
    /// 对应的调用 ID；缺失时由转换校验报告配对失败。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub call_id: OptionalNullable<String>,
    /// 工具输出，允许文本或动态内容。
    pub output: Value,
    /// 固定为 function_call_output。
    pub r#type: ResultType,
    /// 工具结果项 ID。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub id: OptionalNullable<String>,
    /// 处理状态。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub status: OptionalNullable<String>,
    /// 未知协议扩展。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
