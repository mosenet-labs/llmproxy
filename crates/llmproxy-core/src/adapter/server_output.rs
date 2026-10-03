//! 专属服务端工具叶子在解码边界归一化；不序列化整包协议。
use crate::{
    ir::{
        message::OpaquePart,
        server_output::{Kind, ServerOutput},
    },
    protocol::Protocol,
};
use serde_json::{Map, Value};

/// 参考：https://ai.google.dev/gemini-api/docs/code-execution
/// https://developers.openai.com/api/docs/guides/tools-code-interpreter
/// https://platform.claude.com/docs/en/agents-and-tools/tool-use/code-execution-tool
pub(super) fn decode(protocol: Protocol, raw: &Map<String, Value>) -> Option<ServerOutput> {
    let mut output = ServerOutput {
        kind: Kind::Action,
        text: None,
        language: None,
        outcome: None,
        original: Some(OpaquePart {
            protocol,
            data: Value::Object(raw.clone()),
        }),
    };
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    if protocol == Protocol::Gemini {
        if let Some(code) = raw.get("executableCode") {
            output.kind = Kind::Code;
            output.text = text(code, "code");
            output.language = text(code, "language");
        } else {
            let result = raw.get("codeExecutionResult")?;
            output.kind = Kind::ExecutionResult;
            output.text = text(result, "output");
            output.outcome = text(result, "outcome");
        }
    } else {
        match raw.get("type").and_then(Value::as_str)? {
            "code_interpreter_call" => {
                output.kind = Kind::ExecutionResult;
                let mut visible = Vec::new();
                if let Some(code) = raw.get("code").and_then(Value::as_str) {
                    visible.push(code.to_owned());
                }
                for value in raw
                    .get("outputs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(logs) = value.get("logs").and_then(Value::as_str) {
                        visible.push(logs.to_owned());
                    }
                }
                output.text = Some(visible.join("\n"));
            }
            "code_execution_tool_result"
            | "bash_code_execution_tool_result"
            | "text_editor_code_execution_tool_result" => {
                output.kind = Kind::ExecutionResult;
                if let Some(content) = raw.get("content") {
                    let mut visible = Vec::new();
                    for key in ["stdout", "stderr", "content", "error_code"] {
                        if let Some(value) = content.get(key).and_then(Value::as_str) {
                            visible.push(value.to_owned());
                        }
                    }
                    if let Some(code) = content.get("return_code").and_then(Value::as_i64) {
                        visible.push(format!("exit_code: {code}"));
                    }
                    output.text = Some(visible.join("\n"));
                }
            }
            "web_search_tool_result" => {
                output.kind = Kind::SearchResult;
                output.text = raw.get("content").and_then(Value::as_array).map(|results| {
                    results
                        .iter()
                        .filter_map(|r| {
                            let url = r.get("url")?.as_str()?;
                            let title = r.get("title").and_then(Value::as_str).unwrap_or("");
                            Some(format!("{title}\n{url}"))
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            }
            "web_search_call" => {
                output.kind = Kind::SearchResult;
                output.text = raw
                    .get("action")
                    .and_then(|a| a.get("sources"))
                    .and_then(Value::as_array)
                    .map(|sources| {
                        sources
                            .iter()
                            .filter_map(|source| {
                                let url = source.get("url")?.as_str()?;
                                let title =
                                    source.get("title").and_then(Value::as_str).unwrap_or("");
                                Some(format!("{title}\n{url}"))
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
            }
            "server_tool_use" | "file_search_call" => {}
            _ => return None,
        }
    }
    Some(output)
}

/// 同协议消息适配器只有在可见字段未编辑时才可恢复原始执行记录。
pub(super) fn original(output: &ServerOutput, protocol: Protocol) -> super::Result<&Value> {
    let Some(original) = &output.original else {
        return Err(super::Error::Unsupported(
            "服务端输出没有来源副本，请通过整体协议编码器重建".into(),
        ));
    };
    if original.protocol != protocol
        || original
            .data
            .as_object()
            .and_then(|raw| decode(protocol, raw))
            .as_ref()
            != Some(output)
    {
        return Err(super::Error::Unsupported(
            "服务端执行记录不能无损回写编辑或跨协议复制".into(),
        ));
    }
    Ok(&original.data)
}
