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
    /// 服务端显式缓存内容的引用，例如 Gemini 的 `cachedContent`。
    pub reference: Option<String>,
}

/// 一次响应中缓存读取与写入的输入词元数。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheUsage {
    /// 从提示缓存读取的词元数；`None` 表示上游未报告。
    pub read_input_tokens: Option<u64>,
    /// 写入提示缓存的词元数；`None` 表示上游未报告。
    pub write_input_tokens: Option<u64>,
    /// 写入短时缓存的词元数；可用于区分不同 TTL 的计费。
    pub write_short_input_tokens: Option<u64>,
    /// 写入长时缓存的词元数。
    pub write_long_input_tokens: Option<u64>,
}

/// 内容前缀的缓存断点；位置在有序消息或工具声明中，而不是请求末尾。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// 断点所在内容位置。
    pub location: CacheLocation,
    /// 来源显式给出的缓存 TTL。
    pub ttl: Option<String>,
}

/// 不依赖来源协议字段路径的缓存位置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CacheLocation {
    /// 顶层指令序号。
    Instruction(usize),
    /// 普通消息及其内容块序号。
    Message { message: usize, part: usize },
    /// 客户端函数声明序号。
    Tool(usize),
}
