//! 非流式响应的统一用量事件；正文、模型名和签名不进入日志。

use llmproxy_core::{ir::usage::Usage, protocol::Protocol};

/// 在转换前记录 Provider 报告的计数，避免目标协议的表达限制影响统计。
/// 缺失计数保持缺失；缓存读取、写入与推理明细不重复累加到总量。
pub fn response_usage(source: Protocol, target: Protocol, usage: &Usage) {
    tracing::info!(
        component = "gateway",
        event_kind = "usage",
        source = source.as_str(),
        target = target.as_str(),
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        total_tokens = usage.total_tokens,
        cache_read_input_tokens = usage.cache.read_input_tokens,
        cache_write_input_tokens = usage.cache.write_input_tokens,
        cache_write_short_input_tokens = usage.cache.write_short_input_tokens,
        cache_write_long_input_tokens = usage.cache.write_long_input_tokens,
        reasoning_tokens = usage.output_details.reasoning_tokens,
        tool_input_tokens = usage.input_details.tool_tokens,
        "provider response usage"
    );
}
