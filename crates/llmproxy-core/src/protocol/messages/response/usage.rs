//! Messages 响应用量；`input_tokens` 不包含缓存读写部分。
//! 参考 API：https://platform.claude.com/docs/en/api/messages/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::OptionalNullable;

/// 完整响应的用量与缓存明细。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 按不同 TTL 划分的缓存写入量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_creation: OptionalNullable<CacheCreation>,
    /// 写入缓存的输入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_creation_input_tokens: OptionalNullable<u64>,
    /// 从缓存读取的输入词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub cache_read_input_tokens: OptionalNullable<u64>,
    /// 本次使用的推理位置。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub inference_geo: OptionalNullable<String>,
    /// 未缓存的输入词元数；不是全部输入量。
    pub input_tokens: u64,
    /// 输出词元数。
    pub output_tokens: u64,
    /// 输出词元细项。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub output_tokens_details: OptionalNullable<OutputTokensDetails>,
    /// 服务端工具调用次数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub server_tool_use: OptionalNullable<ServerToolUse>,
    /// 实际服务层级。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub service_tier: OptionalNullable<String>,
    /// 保留新增用量字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 不同存活时间的缓存写入词元数。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheCreation {
    /// 一小时缓存写入量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub ephemeral_1h_input_tokens: OptionalNullable<u64>,
    /// 五分钟缓存写入量。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub ephemeral_5m_input_tokens: OptionalNullable<u64>,
    /// 保留新增缓存统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输出词元细项。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputTokensDetails {
    /// 模型思考使用的词元数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub thinking_tokens: OptionalNullable<u64>,
    /// 保留新增输出统计。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务端工具请求次数，不属于词元数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerToolUse {
    /// 网页抓取请求数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub web_fetch_requests: OptionalNullable<u64>,
    /// 网页搜索请求数。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub web_search_requests: OptionalNullable<u64>,
    /// 保留其他工具请求数。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
