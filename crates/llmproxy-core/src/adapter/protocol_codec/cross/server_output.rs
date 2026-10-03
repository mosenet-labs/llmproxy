//! 服务端工具执行结果转为可见正文，不制造需要客户端执行的调用。
use super::{ConversionWarning, warn};
use crate::{
    ir::server_output::{Kind, ServerOutput},
    protocol::Protocol,
};

/// 仅访问已归一化字段；执行 ID、容器引用、签名及原始副本不进入目标报文。
pub(super) fn text(
    output: &ServerOutput,
    source: Protocol,
    target: Protocol,
    path: &str,
    warnings: &mut Vec<ConversionWarning>,
) -> Option<String> {
    warn(
        warnings,
        source,
        target,
        path,
        "服务端执行记录仅保留可见正文，专属执行状态、引用及附件不跨 Provider 复制",
    );
    let text = output.text.as_ref().filter(|text| !text.is_empty())?;
    Some(match output.kind {
        Kind::Code => format!(
            "代码（{}）:\n{text}",
            output.language.as_deref().unwrap_or("未指定语言")
        ),
        Kind::ExecutionResult => match &output.outcome {
            Some(outcome) => format!("执行结果（{outcome}）:\n{text}"),
            None => text.clone(),
        },
        Kind::SearchResult | Kind::Action => text.clone(),
    })
}
