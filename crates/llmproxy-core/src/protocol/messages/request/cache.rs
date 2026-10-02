//! Messages 请求级及内容块级提示缓存断点。
//! 参考 API：https://platform.claude.com/docs/en/api/messages/create

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::OptionalNullable;

/// 临时提示缓存断点；TTL 通常为 `5m` 或 `1h`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CacheControl {
    /// 断点类型，标准值为 `ephemeral`。
    pub r#type: String,
    /// 断点的存活时间；省略时使用服务端默认值。
    #[serde(default, skip_serializing_if = "OptionalNullable::is_missing")]
    pub ttl: OptionalNullable<String>,
    /// 保留新增缓存参数。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
