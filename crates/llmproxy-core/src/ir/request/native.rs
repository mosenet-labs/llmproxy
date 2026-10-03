//! Provider 执行的内置能力，与客户端函数声明分开。
use serde::{Deserialize, Serialize};

/// 可跨协议重新构造的服务端工具；不包含私有容器和文件引用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NativeTool {
    /// 联网搜索；过滤条件不能在转换中被扩大。
    WebSearch(WebSearch),
    /// 由目标 Provider 创建新的代码执行环境。
    CodeExecution,
}
/// 搜索的公共配置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WebSearch {
    /// 仅允许这些域名；空列表表示没有白名单。
    pub allowed_domains: Vec<String>,
    /// 禁止搜索这些域名。
    pub blocked_domains: Vec<String>,
    /// 搜索上下文量；目标没有此选项时显式降级。
    pub context_size: Option<String>,
    /// 用户提供的近似位置。
    pub location: Option<Location>,
    /// false 表示只使用缓存搜索，不能降为任意联网搜索。
    pub external_access: Option<bool>,
}
/// 不依赖协议包装方式的近似位置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// 城市。
    pub city: Option<String>,
    /// 国家代码。
    pub country: Option<String>,
    /// 区域。
    pub region: Option<String>,
    /// IANA 时区。
    pub timezone: Option<String>,
}
