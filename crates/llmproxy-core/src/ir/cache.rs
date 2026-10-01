//! 请求的提示缓存配置，以及响应中可统计的缓存用量。

use serde::{Deserialize, Serialize};

/// 与协议无关的提示缓存配置；缺失值表示客户端未显式指定。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheSettings {
    /// 用于缓存分组或匹配的键。
    pub key: Option<String>,
    /// 隐式或显式缓存断点模式。
    pub mode: Option<String>,
    /// 缓存条目的最低生存时间。
    pub ttl: Option<String>,
    /// 旧版缓存保留策略。
    pub retention: Option<String>,
}

/// 一次响应中缓存读取与写入的输入词元数。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheUsage {
    /// 从提示缓存读取的词元数；`None` 表示上游未报告。
    pub read_input_tokens: Option<u64>,
    /// 写入提示缓存的词元数；`None` 表示上游未报告。
    pub write_input_tokens: Option<u64>,
}
