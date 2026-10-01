//! Chat Completions 请求参数中的固定子对象。
//! 参考 API：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::message::TextPart;
use crate::protocol::optional_nullable::OptionalNullable;

/// 音频输出的格式和声音。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioOutput {
    /// 输出音频格式，例如 `wav`、`mp3` 或 `pcm16`。
    pub format: String,
    /// 内置声音名称或自定义声音 ID。
    pub voice: Voice,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 内置声音名称或自定义声音引用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Voice {
    /// 内置声音名称。
    Name(String),
    /// 自定义声音对象。
    Custom(CustomVoice),
}

/// 自定义声音的引用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomVoice {
    /// 自定义声音 ID。
    pub id: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 旧版函数调用选择：`none`、`auto` 或指定函数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FunctionChoice {
    /// `none` 或 `auto`。
    Mode(String),
    /// 指定函数名称。
    Named(NamedTool),
}

/// 通过名称指定一个函数或自定义工具。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedTool {
    /// 要调用的工具名称。
    pub name: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 函数工具定义；`parameters` 是调用方提供的任意 JSON Schema。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// 函数名称。
    pub name: String,
    /// 用于模型选择函数的说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 函数参数的 JSON Schema。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub parameters: OptionalNullable<Value>,
    /// 是否严格遵循参数结构；旧版 `functions` 可省略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub strict: OptionalNullable<bool>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入和输出审核的配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModerationSettings {
    /// 用于审核的模型 ID。
    pub model: String,
    /// 输入和输出各自的审核策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub policy: OptionalNullable<ModerationPolicy>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入与输出的审核策略。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModerationPolicy {
    /// 输入审核策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub input: OptionalNullable<ModerationMode>,
    /// 输出审核策略。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub output: OptionalNullable<ModerationMode>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一侧审核的处理方式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModerationMode {
    /// `score` 只提供分数，`block` 允许阻断。
    pub mode: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 预测式输出的已知内容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// 一段文本或文本内容块数组。
    pub content: PredictionContent,
    /// 预测类型，标准值为 `content`。
    pub r#type: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 预测式输出支持的两种内容形状。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PredictionContent {
    /// 单段文本。
    Text(String),
    /// 带类型的文本内容块。
    Parts(Vec<TextPart>),
}

/// 提示缓存模式与缓存条目的最低生存时间。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PromptCacheOptions {
    /// `implicit` 或 `explicit` 缓存断点模式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub mode: OptionalNullable<String>,
    /// 缓存条目的最低生存时间，例如 `30m`。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub ttl: OptionalNullable<String>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出格式：文本、JSON 对象或 JSON Schema。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// 普通文本输出。
    Text {
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 旧版 JSON 对象输出。
    JsonObject {
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 按调用方提供的 JSON Schema 输出。
    JsonSchema {
        /// JSON Schema 配置。
        json_schema: JsonSchemaFormat,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 结构化输出所用的 JSON Schema 配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonSchemaFormat {
    /// 格式名称。
    pub name: String,
    /// 格式用途的说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 调用方提供的任意 JSON Schema 对象。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub schema: OptionalNullable<Map<String, Value>>,
    /// 是否要求模型严格遵循 Schema。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub strict: OptionalNullable<bool>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 一个停止序列或一组停止序列。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopSequences {
    /// 单个停止序列。
    One(String),
    /// 多个停止序列。
    Many(Vec<String>),
}

/// 工具调用选择：模式字符串或具名工具配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// `none`、`auto` 或 `required`。
    Mode(String),
    /// 指定工具或允许工具集合。
    Specific(SpecificToolChoice),
}

/// 结构化工具调用选择。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpecificToolChoice {
    /// 指定函数工具。
    Function {
        /// 函数名称。
        function: NamedTool,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 指定自定义工具。
    Custom {
        /// 自定义工具名称。
        custom: NamedTool,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 只允许调用给定工具集合。
    AllowedTools {
        /// 允许的工具及选择模式。
        allowed_tools: AllowedTools,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 允许工具集合中的模式和工具定义。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AllowedTools {
    /// `auto` 或 `required`。
    pub mode: String,
    /// 允许的工具；文档把每项定义为开放 JSON 对象。
    pub tools: Vec<Map<String, Value>>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 模型可调用的函数工具或自定义工具。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tool {
    /// 函数工具定义。
    Function {
        /// 函数名称、说明与参数结构。
        function: FunctionDefinition,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 自定义工具定义。
    Custom {
        /// 自定义工具名称及输入格式。
        custom: CustomTool,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 自定义工具的输入定义。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomTool {
    /// 工具名称。
    pub name: String,
    /// 工具用途说明。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub description: OptionalNullable<String>,
    /// 自由文本或语法约束格式。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub format: OptionalNullable<CustomToolFormat>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 自定义工具接受的输入格式。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CustomToolFormat {
    /// 不限制的自由文本。
    Text {
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// 按 Lark 或正则语法约束输入。
    Grammar {
        /// 语法定义及语法类型。
        grammar: Grammar,
        /// 保留供应商扩展字段。
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// 自定义工具输入语法。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grammar {
    /// 语法正文。
    pub definition: String,
    /// `lark` 或 `regex`。
    pub syntax: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 网页搜索的上下文量和用户位置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebSearchOptions {
    /// `low`、`medium` 或 `high` 的搜索上下文量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub search_context_size: OptionalNullable<String>,
    /// 用户的大致位置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub user_location: OptionalNullable<UserLocation>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 网页搜索的位置配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserLocation {
    /// 近似位置字段。
    pub approximate: ApproximateLocation,
    /// 位置类型，标准值为 `approximate`。
    pub r#type: String,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 用于网页搜索的非精确地理位置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApproximateLocation {
    /// 城市名称。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub city: OptionalNullable<String>,
    /// 两位国家代码。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub country: OptionalNullable<String>,
    /// 地区名称。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub region: OptionalNullable<String>,
    /// IANA 时区名称。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub timezone: OptionalNullable<String>,
    /// 保留供应商扩展字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
