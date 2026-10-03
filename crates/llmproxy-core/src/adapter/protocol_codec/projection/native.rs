//! 内置工具仅在解码边界解释局部扩展对象，编码端只接收公共类型。
use super::Notes;
use crate::{
    ir::request::native::{Location, NativeTool, WebSearch},
    protocol::{
        Request, chat::request::parameters::Tool as C, messages::request::tool::Tool as M,
        responses::function::Tool as R,
    },
};
use serde_json::{Map, Value};

/// 参考：https://developers.openai.com/api/docs/guides/tools-web-search
/// https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool
/// https://ai.google.dev/gemini-api/docs/google-search
pub(super) fn decode(source: &Request, notes: &mut Notes) -> Vec<NativeTool> {
    let mut output = Vec::new();
    match source {
        Request::Chat(b) => {
            if let Some(search) = b.web_search_options.as_option() {
                notes.extra(&search.extra, "web_search_options");
                let location = search.user_location.as_option().map(|location| {
                    notes.extra(&location.extra, "web_search_options.user_location");
                    notes.extra(
                        &location.approximate.extra,
                        "web_search_options.user_location.approximate",
                    );
                    let l = &location.approximate;
                    Location {
                        city: l.city.as_option().cloned(),
                        country: l.country.as_option().cloned(),
                        region: l.region.as_option().cloned(),
                        timezone: l.timezone.as_option().cloned(),
                    }
                });
                output.push(NativeTool::WebSearch(WebSearch {
                    context_size: search.search_context_size.as_option().cloned(),
                    location,
                    ..Default::default()
                }));
            }
            for (i, tool) in b.tools.as_option().into_iter().flatten().enumerate() {
                if !matches!(tool, C::Function { .. }) {
                    notes.dropped(format!("tools[{i}].custom"));
                }
            }
        }
        Request::Responses(b) => {
            for (i, tool) in b.tools.as_option().into_iter().flatten().enumerate() {
                if let R::Other(tool) = tool {
                    object(tool, &mut output, notes, &format!("tools[{i}]"));
                }
            }
        }
        Request::Messages(b) => {
            for (i, tool) in b.tools.as_option().into_iter().flatten().enumerate() {
                match tool {
                    M::Other(tool) => object(tool, &mut output, notes, &format!("tools[{i}]")),
                    M::Function(f) if f.r#type.as_option().is_some_and(|t| t != "custom") => {
                        notes.dropped(format!("tools[{i}].type"))
                    }
                    _ => {}
                }
            }
        }
        Request::Gemini(b) => {
            for (i, tool) in b.tools.as_option().into_iter().flatten().enumerate() {
                let path = format!("tools[{i}]");
                if let Some(config) = tool.google_search.as_option() {
                    let blocked_domains = list(config.get("excludeDomains"), notes, &path);
                    let mut extra = config.clone();
                    extra.remove("excludeDomains");
                    for key in extra.keys() {
                        notes.reject(
                            &format!("{path}.googleSearch.{key}"),
                            "目标无法等价保持搜索类型或过滤条件",
                        );
                    }
                    output.push(NativeTool::WebSearch(WebSearch {
                        blocked_domains,
                        ..Default::default()
                    }));
                }
                if let Some(config) = tool.code_execution.as_option() {
                    notes.extra(config, &format!("{path}.codeExecution"));
                    output.push(NativeTool::CodeExecution);
                }
                notes.field(
                    &tool.google_search_retrieval,
                    &format!("{path}.googleSearchRetrieval"),
                );
                notes.field(&tool.computer_use, &format!("{path}.computerUse"));
                notes.field(&tool.url_context, &format!("{path}.urlContext"));
                if tool.file_search.as_option().is_some() {
                    notes.reject(
                        &format!("{path}.fileSearch"),
                        "文件搜索依赖来源 Provider 的文件库，必须先物化上下文",
                    );
                }
                notes.field(&tool.mcp_servers, &format!("{path}.mcpServers"));
                notes.field(&tool.google_maps, &format!("{path}.googleMaps"));
            }
        }
    }
    output
}
/// 不转换容器 ID、文件库 ID 或执行凭据；未知工具按约定告警丢弃。
fn object(tool: &Map<String, Value>, output: &mut Vec<NativeTool>, notes: &mut Notes, path: &str) {
    let kind = tool.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(
        kind,
        "web_search" | "web_search_preview" | "web_search_preview_2025_03_11"
    ) || kind.starts_with("web_search_")
    {
        if tool.get("filters").is_some_and(|value| !value.is_object()) {
            notes.reject(path, "搜索过滤配置必须是对象");
        }
        let filters = tool
            .get("filters")
            .and_then(Value::as_object)
            .unwrap_or(tool);
        let mut search = WebSearch {
            allowed_domains: list(filters.get("allowed_domains"), notes, path),
            blocked_domains: list(filters.get("blocked_domains"), notes, path),
            ..Default::default()
        };
        search.context_size = tool
            .get("search_context_size")
            .and_then(Value::as_str)
            .map(str::to_owned);
        search.external_access = tool.get("external_web_access").and_then(Value::as_bool);
        if tool
            .get("external_web_access")
            .is_some_and(|v| !v.is_boolean())
        {
            notes.reject(path, "external_web_access 必须是布尔值");
        }
        if let Some(location) = tool.get("user_location").and_then(Value::as_object) {
            let field = |key| location.get(key).and_then(Value::as_str).map(str::to_owned);
            search.location = Some(Location {
                city: field("city"),
                country: field("country"),
                region: field("region"),
                timezone: field("timezone"),
            });
            for key in location.keys().filter(|k| {
                !matches!(
                    k.as_str(),
                    "type" | "city" | "country" | "region" | "timezone"
                )
            }) {
                notes.dropped(format!("{path}.user_location.{key}"));
            }
        }
        for key in tool.keys().filter(|k| {
            !matches!(
                k.as_str(),
                "type"
                    | "name"
                    | "filters"
                    | "allowed_domains"
                    | "blocked_domains"
                    | "search_context_size"
                    | "external_web_access"
                    | "user_location"
            )
        }) {
            notes.dropped(format!("{path}.{key}"));
        }
        if let Some(filters) = tool.get("filters").and_then(Value::as_object) {
            for key in filters
                .keys()
                .filter(|k| !matches!(k.as_str(), "allowed_domains" | "blocked_domains"))
            {
                notes.reject(&format!("{path}.filters.{key}"), "无法等价转换搜索过滤条件");
            }
        }
        output.push(NativeTool::WebSearch(search));
    } else if kind == "code_interpreter" || kind.starts_with("code_execution_") {
        if let Some(container) = tool.get("container") {
            if container.get("type").and_then(Value::as_str) != Some("auto")
                || container
                    .get("file_ids")
                    .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
            {
                notes.reject(path, "代码执行容器或文件属于来源 Provider，必须先物化");
            }
            if let Some(config) = container.as_object() {
                for key in config
                    .keys()
                    .filter(|k| !matches!(k.as_str(), "type" | "file_ids"))
                {
                    notes.dropped(format!("{path}.container.{key}"));
                }
            }
        }
        for key in tool
            .keys()
            .filter(|k| !matches!(k.as_str(), "type" | "name" | "container"))
        {
            notes.dropped(format!("{path}.{key}"));
        }
        output.push(NativeTool::CodeExecution);
    } else if kind == "file_search" {
        notes.reject(path, "文件搜索依赖来源 Provider 的文件库，必须先物化上下文");
    } else {
        notes.dropped(path);
    }
}
/// 过滤条件格式错误时拒绝转换，不能悄悄变成无约束搜索。
fn list(value: Option<&Value>, notes: &mut Notes, path: &str) -> Vec<String> {
    match value {
        None => vec![],
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => items
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect(),
        _ => {
            notes.reject(path, "域名过滤条件必须是字符串数组");
            vec![]
        }
    }
}
