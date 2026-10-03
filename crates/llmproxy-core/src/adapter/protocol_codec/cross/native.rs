//! 从通用内置能力构造目标声明；不复制来源工具对象。
use super::{ConversionWarning, unsupported, warn};
use crate::{
    adapter::Result,
    ir::request::{
        Request as IrRequest,
        controls::ToolChoice,
        native::{NativeTool, WebSearch},
    },
    protocol::{
        OptionalNullable as O, Protocol, Request, chat::request::parameters as c,
        gemini::request::body as g, messages::request::tool as m, responses::function as r,
    },
};
use serde_json::{Map, Value, json};

/// 参考：https://developers.openai.com/api/docs/guides/tools-code-interpreter
/// https://platform.claude.com/docs/en/agents-and-tools/tool-use/code-execution-tool
/// https://ai.google.dev/gemini-api/docs/code-execution
pub(super) fn encode(
    request: &IrRequest,
    body: &mut Request,
    warnings: &mut Vec<ConversionWarning>,
) -> Result<()> {
    let source = request.source_protocol();
    let target = body.protocol();
    let mut seen = Vec::new();
    for tool in &request.native_tools {
        let kind = std::mem::discriminant(tool);
        if seen.contains(&kind) {
            return Err(unsupported("native_tools", "同一内置能力不能重复声明"));
        }
        seen.push(kind);
        if matches!(
            request.generation.tool_choice,
            Some(ToolChoice::None | ToolChoice::Named(_) | ToolChoice::Allowed { .. })
        ) {
            warn(
                warnings,
                source,
                target,
                "native_tools",
                "调用约束仅允许客户端函数或禁止调用，未启用内置工具",
            );
            continue;
        }
        match tool {
            NativeTool::WebSearch(search) => {
                search_constraints(search, target)?;
                match body {
                    Request::Chat(b) => {
                        b.web_search_options = O::Value(c::WebSearchOptions {
                            search_context_size: search.context_size.clone().into(),
                            user_location: search
                                .location
                                .as_ref()
                                .map(|l| c::UserLocation {
                                    r#type: "approximate".into(),
                                    approximate: c::ApproximateLocation {
                                        city: l.city.clone().into(),
                                        country: l.country.clone().into(),
                                        region: l.region.clone().into(),
                                        timezone: l.timezone.clone().into(),
                                        extra: Map::new(),
                                    },
                                    extra: Map::new(),
                                })
                                .into(),
                            extra: Map::new(),
                        })
                    }
                    Request::Responses(b) => {
                        let mut tool = Map::from_iter([("type".into(), json!("web_search"))]);
                        let mut filters = Map::new();
                        if !search.allowed_domains.is_empty() {
                            filters.insert("allowed_domains".into(), json!(search.allowed_domains));
                        }
                        if !search.blocked_domains.is_empty() {
                            filters.insert("blocked_domains".into(), json!(search.blocked_domains));
                        }
                        if !filters.is_empty() {
                            tool.insert("filters".into(), Value::Object(filters));
                        }
                        if let Some(size) = &search.context_size {
                            tool.insert("search_context_size".into(), json!(size));
                        }
                        if let Some(access) = search.external_access {
                            tool.insert("external_web_access".into(), json!(access));
                        }
                        if let Some(l) = &search.location {
                            tool.insert("user_location".into(), location(l));
                        }
                        push(&mut b.tools, r::Tool::Other(tool));
                    }
                    Request::Messages(b) => {
                        let mut tool = Map::from_iter([
                            ("type".into(), json!("web_search_20250305")),
                            ("name".into(), json!("web_search")),
                        ]);
                        if !search.allowed_domains.is_empty() {
                            tool.insert("allowed_domains".into(), json!(search.allowed_domains));
                        }
                        if !search.blocked_domains.is_empty() {
                            tool.insert("blocked_domains".into(), json!(search.blocked_domains));
                        }
                        if let Some(l) = &search.location {
                            tool.insert("user_location".into(), location(l));
                        }
                        push(&mut b.tools, m::Tool::Other(tool));
                    }
                    Request::Gemini(b) => {
                        push(
                            &mut b.tools,
                            g::Tool {
                                google_search: O::Value(Map::new()),
                                ..Default::default()
                            },
                        );
                    }
                }
                if search.context_size.is_some()
                    && matches!(target, Protocol::AnthropicMessages | Protocol::Gemini)
                {
                    warn(
                        warnings,
                        source,
                        target,
                        "native_tools.web_search.context_size",
                        "目标无搜索上下文量设置，使用目标默认值",
                    );
                }
                if search.location.is_some() && target == Protocol::Gemini {
                    warn(
                        warnings,
                        source,
                        target,
                        "native_tools.web_search.location",
                        "目标搜索声明无对应位置字段，已丢弃",
                    );
                }
                warn(
                    warnings,
                    source,
                    target,
                    "native_tools.web_search",
                    "搜索引擎与结果排序由目标 Provider 决定",
                );
            }
            NativeTool::CodeExecution => {
                match body {
                    Request::Chat(_) => {
                        warn(
                            warnings,
                            source,
                            target,
                            "native_tools.code_execution",
                            "Chat 没有内置代码执行声明，已丢弃此工具",
                        );
                        continue;
                    }
                    Request::Responses(b) => push(
                        &mut b.tools,
                        r::Tool::Other(
                            json!({"type":"code_interpreter","container":{"type":"auto"}})
                                .as_object()
                                .unwrap()
                                .clone(),
                        ),
                    ),
                    Request::Messages(b) => push(
                        &mut b.tools,
                        m::Tool::Other(
                            json!({"type":"code_execution_20250825","name":"code_execution"})
                                .as_object()
                                .unwrap()
                                .clone(),
                        ),
                    ),
                    Request::Gemini(b) => push(
                        &mut b.tools,
                        g::Tool {
                            code_execution: O::Value(Map::new()),
                            ..Default::default()
                        },
                    ),
                }
                warn(
                    warnings,
                    source,
                    target,
                    "native_tools.code_execution",
                    "在目标 Provider 新建执行环境；运行时、依赖和资源上限可能不同",
                );
            }
        }
    }
    Ok(())
}
/// 能力声明缺失时创建列表，保留已有客户端函数的相对位置。
fn push<T>(field: &mut O<Vec<T>>, item: T) {
    if let O::Value(items) = field {
        items.push(item);
    } else {
        *field = O::Value(vec![item]);
    }
}
/// 域名和离线约束无法保持时拒绝，避免把受限搜索变为开放搜索。
fn search_constraints(search: &WebSearch, target: Protocol) -> Result<()> {
    if (!search.allowed_domains.is_empty()
        && matches!(target, Protocol::OpenAiChat | Protocol::Gemini))
        || (!search.blocked_domains.is_empty()
            && matches!(target, Protocol::OpenAiChat | Protocol::Gemini))
    {
        return Err(unsupported(
            "native_tools.web_search.domains",
            "目标无法表达域名限制",
        ));
    }
    if target == Protocol::AnthropicMessages
        && !search.allowed_domains.is_empty()
        && !search.blocked_domains.is_empty()
    {
        return Err(unsupported(
            "native_tools.web_search.domains",
            "Messages 不能同时设置允许和禁止域名",
        ));
    }
    if search.external_access == Some(false) && target != Protocol::OpenAiResponses {
        return Err(unsupported(
            "native_tools.web_search.external_access",
            "目标无法保证仅使用缓存搜索",
        ));
    }
    Ok(())
}
/// 两种服务端工具使用同形的近似位置叶子，不携带来源扩展字段。
fn location(l: &crate::ir::request::native::Location) -> Value {
    let mut fields = Map::from_iter([("type".into(), json!("approximate"))]);
    for (key, value) in [
        ("city", &l.city),
        ("country", &l.country),
        ("region", &l.region),
        ("timezone", &l.timezone),
    ] {
        if let Some(value) = value {
            fields.insert(key.into(), json!(value));
        }
    }
    Value::Object(fields)
}
