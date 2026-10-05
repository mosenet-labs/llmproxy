//! 同协议直接编辑协议类型，保留未改动的 null、缺失值和专有字段。
use super::{cross, project_request, project_response};
use crate::{
    adapter::{Error, Result},
    ir::{request::Request, response::Response},
    protocol::{Protocol, Request as RawRequest, Response as RawResponse},
};

/// 保留模式要求来源协议一致，消息、缓存及公共字段共用一次来源基线。
pub(super) fn encode_request(protocol: Protocol, edited: &Request) -> Result<RawRequest> {
    let original = edited
        .source
        .as_ref()
        .filter(|body| body.protocol() == protocol)
        .ok_or_else(|| Error::Unsupported("保留模式要求同协议来源副本".into()))?;
    let before = project_request(original)?;
    if edited.instructions != before.instructions || edited.items != before.items {
        return Err(Error::Unsupported(
            "保留模式不支持修改顶层指令或独立输入项，请使用重建模式".into(),
        ));
    }
    let mut body = original.clone();
    super::request::encode_messages(&mut body, &edited.messages, &before.messages)?;
    if edited.cache != before.cache {
        super::request::encode_cache(&mut body, &edited.cache);
        if super::request::decode_cache(&body) != edited.cache {
            return Err(Error::Unsupported(
                "目标协议无法表达修改后的缓存设置".into(),
            ));
        }
    }
    request(protocol, edited, &before, &mut body)?;
    Ok(body)
}

/// 响应编辑复用来源用量和候选基线，只有返回正文需要拥有来源副本。
pub(super) fn encode_response(protocol: Protocol, edited: &Response) -> Result<RawResponse> {
    let original = edited
        .source
        .as_ref()
        .filter(|body| body.protocol() == protocol)
        .ok_or_else(|| Error::Unsupported("保留模式要求同协议来源副本".into()))?;
    let before = project_response(original)?;
    if edited.items != before.items {
        return Err(Error::Unsupported(
            "保留模式不支持修改独立输出项，请使用重建模式".into(),
        ));
    }
    let mut body = original.clone();
    super::response::encode_messages(&mut body, &edited.messages, &before.messages)?;
    if edited.usage != before.usage {
        super::response::encode_usage(&mut body, edited.usage.as_ref())?;
        if !super::response::changed_usage_fields_match(
            &before.usage,
            &edited.usage,
            &super::response::decode_usage(&body),
        ) {
            return Err(Error::Unsupported("目标协议无法表达修改后的用量".into()));
        }
    }
    response(edited, &before, &mut body)?;
    Ok(body)
}

/// 新增通用请求字段只回写实际变化的部分。
pub(super) fn request(
    protocol: Protocol,
    edited: &Request,
    before: &Request,
    body: &mut RawRequest,
) -> Result<()> {
    if edited.native_tools != before.native_tools {
        return Err(Error::Unsupported("编辑内置工具时请使用重建模式".into()));
    }
    if edited.cache_breakpoints != before.cache_breakpoints {
        return Err(Error::Unsupported("修改断点位置时请使用重建模式".into()));
    }
    if edited.model != before.model {
        match body {
            RawRequest::Chat(b) => b.model = required(&edited.model, "model")?,
            RawRequest::Responses(b) => b.model = edited.model.clone().into(),
            RawRequest::Messages(b) => b.model = required(&edited.model, "model")?,
            RawRequest::Gemini(_) => {
                return Err(Error::Unsupported(
                    "Gemini 模型位于 URL，不能写入请求正文".into(),
                ));
            }
        }
    }
    if edited.tools != before.tools {
        if before
            .diagnostics
            .iter()
            .any(|note| note.reject || note.path.starts_with("tools"))
        {
            return Err(Error::Unsupported(
                "工具声明含未规范化字段，不能无损回写编辑".into(),
            ));
        }
        let mut warnings = vec![];
        cross::tools::encode_tools(protocol, body, &edited.tools, &mut warnings)?;
        if !warnings.is_empty() {
            return Err(Error::Unsupported(
                "目标协议无法无损表达编辑后的工具声明".into(),
            ));
        }
    }
    if edited.generation != before.generation {
        // 校验在副本上进行，实际回写保留未编辑字段的原始表示。
        let mut checked = body.clone();
        let mut warnings = vec![];
        cross::generation::encode(
            protocol,
            protocol,
            &edited.generation,
            &mut checked,
            &mut warnings,
        )?;
        if !warnings.is_empty() {
            return Err(Error::Unsupported(
                "目标协议无法无损表达编辑后的生成参数".into(),
            ));
        }
        cross::generation::write(body, &edited.generation, Some(&before.generation))?;
    }
    let mut actual = project_request(body)?;
    if protocol == Protocol::Gemini {
        // 流式模式属于 URL，调用方继续持有 IR 中的模式；不能用正文解码结果否定它。
        actual.generation.stream = edited.generation.stream;
    }
    if actual.model != edited.model
        || actual.native_tools != edited.native_tools
        || actual.tools != edited.tools
        || actual.generation != edited.generation
    {
        return Err(Error::Unsupported(
            "目标协议无法表达编辑后的请求字段".into(),
        ));
    }
    Ok(())
}
/// 响应外壳直接回写；未实现的候选结构编辑仍明确拒绝。
pub(super) fn response(edited: &Response, before: &Response, body: &mut RawResponse) -> Result<()> {
    if edited.status != before.status || edited.candidates != before.candidates {
        return Err(Error::Unsupported(
            "同协议回写暂不支持修改状态或候选边界".into(),
        ));
    }
    if edited.failure != before.failure {
        if edited.failure.is_some() && edited.status != crate::ir::response::Status::Failed {
            return Err(Error::Unsupported("非失败状态不能加入生成错误".into()));
        }
        match body {
            RawResponse::Responses(body) => {
                let original = body.error.as_option().cloned();
                body.error = edited
                    .failure
                    .as_ref()
                    .map(|failure| {
                        let mut error = original.clone().unwrap_or(
                            crate::protocol::responses::response::body::ResponseError {
                                code: String::new(),
                                message: String::new(),
                                misalignment: Default::default(),
                                extra: Default::default(),
                            },
                        );
                        error.code.clone_from(&failure.code);
                        error.message.clone_from(&failure.message);
                        error
                    })
                    .into();
            }
            _ => return Err(Error::Unsupported("目标完成响应没有生成错误字段".into())),
        }
    }
    if edited.model != before.model {
        match body {
            RawResponse::Chat(b) => b.model = required(&edited.model, "model")?,
            RawResponse::Responses(b) => b.model = required(&edited.model, "model")?,
            RawResponse::Messages(b) => b.model = required(&edited.model, "model")?,
            RawResponse::Gemini(b) => b.model_version = edited.model.clone().into(),
        }
    }
    if edited.id != before.id {
        match body {
            RawResponse::Chat(b) => b.id = required(&edited.id, "id")?,
            RawResponse::Responses(b) => b.id = required(&edited.id, "id")?,
            RawResponse::Messages(b) => b.id = required(&edited.id, "id")?,
            RawResponse::Gemini(b) => b.response_id = edited.id.clone().into(),
        }
    }
    if edited.created_at != before.created_at {
        let time = edited
            .created_at
            .ok_or_else(|| Error::Unsupported("目标响应必须包含创建时间".into()))?;
        match body {
            RawResponse::Chat(b) => b.created = time,
            RawResponse::Responses(b) => b.created_at = time,
            _ => return Err(Error::Unsupported("目标响应没有创建时间字段".into())),
        }
    }
    Ok(())
}
/// 必填协议字段不能通过 IR 删除。
fn required(value: &Option<String>, path: &str) -> Result<String> {
    value
        .clone()
        .ok_or_else(|| Error::Unsupported(format!("目标协议必须包含 {path}")))
}
