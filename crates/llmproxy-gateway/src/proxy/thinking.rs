//! 网关专用选择头与实际模型校验；Pingora 回调不解释供应商思考字段。
use super::RequestContext;
use llmproxy_core::{
    ir::request::Request,
    thinking::{Choice, HEADER},
};
use pingora::{Error, ErrorType, Result};
use pingora_http::RequestHeader;

/// 拒绝重复头与未知值，避免代理合并后的意图不一致。
pub(super) fn choice(header: &RequestHeader) -> std::result::Result<Choice, &'static str> {
    let mut values = header.headers.get_all(HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(Choice::Default);
    };
    if values.next().is_some() {
        return Err("思考设置请求头不能重复");
    }
    value
        .to_str()
        .ok()
        .and_then(Choice::parse)
        .ok_or("思考设置必须为 default、enabled 或 disabled")
}

/// 在目标编码之前应用当前轮选择；错误仅保留固定说明。
pub(super) fn apply(ctx: &mut RequestContext, request: &mut Request) -> Result<()> {
    let route = ctx.route.as_ref().expect("thinking route selected");
    if let Err(message) = route
        .thinking
        .apply(ctx.thinking, route.provider.protocol, request)
    {
        ctx.thinking_error = Some(message);
        return Err(Error::explain(ErrorType::HTTPStatus(422), message));
    }
    Ok(())
}
