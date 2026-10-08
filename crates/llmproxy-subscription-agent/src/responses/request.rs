use llmproxy_core::protocol::{
    OptionalNullable as O,
    responses::request::{
        Request,
        body::{Input, InputItem},
        message::{Content, EasyInputMessage, Message, Role},
    },
};

/// 正文只在请求内存中存在；上游固定 stream:true，对外模式分别保留。
pub struct Prepared {
    pub upstream: Request,
    pub downstream_stream: bool,
}

#[derive(Debug, PartialEq)]
pub struct RequestError {
    pub param: &'static str,
    pub message: &'static str,
}

fn invalid(param: &'static str, message: &'static str) -> RequestError {
    RequestError { param, message }
}

/// 无状态代理不能接受需要保存正文或依赖服务器会话的参数。
pub fn validate_stateless(request: &Request) -> Result<(), RequestError> {
    if matches!(request.store, O::Value(true)) {
        return Err(invalid("store", "订阅代理不保存响应；store 必须为 false"));
    }
    for (param, present) in [
        (
            "previous_response_id",
            !request.previous_response_id.is_missing(),
        ),
        ("conversation", !request.conversation.is_missing()),
        ("background", !request.background.is_missing()),
    ] {
        if present {
            return Err(invalid(
                param,
                "无状态代理不支持此参数；请在 input 中提交完整上下文",
            ));
        }
    }
    Ok(())
}

/// 官方 SIWC HTTP 要求 store:false、stream:true、数组历史。
/// 未支持的参数报错，剩余类型化字段及扩展原样交给上游验证。
pub fn prepare_oauth_request(request: Request) -> Result<Prepared, RequestError> {
    prepare_request(request, true)
}

pub fn prepare_codex_request(request: Request) -> Result<Prepared, RequestError> {
    let prepared = prepare_request(request, false)?;
    let mut value = serde_json::to_value(prepared.upstream)
        .map_err(|_| invalid("request", "请求无法序列化"))?;
    // 原生 Responses 使用规范数组消息；其他字段与工具 ID 保持原值。
    for item in value["input"].as_array_mut().unwrap() {
        if item.get("role").is_some() {
            if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
                item["content"] = if item["role"] == "assistant" {
                    serde_json::json!([{"type":"output_text","text":text,"annotations":[]}])
                } else {
                    serde_json::json!([{"type":"input_text","text":text}])
                };
            }
            if item.get("type").is_none() {
                item["type"] = serde_json::json!("message");
            }
        }
    }
    if value.get("instructions").is_none() {
        value["instructions"] = serde_json::json!("");
    }
    if serde_json::to_vec(&value)
        .map_err(|_| invalid("request", "请求无法序列化"))?
        .len()
        > crate::service::FRAME_LIMIT - 128
    {
        return Err(invalid("input", "规范化请求及 RPC 封装超过容量上限"));
    }
    Ok(Prepared {
        upstream: serde_json::from_value(value)
            .map_err(|_| invalid("input", "原生 Responses 消息无效"))?,
        downstream_stream: prepared.downstream_stream,
    })
}

fn prepare_request(mut request: Request, oauth: bool) -> Result<Prepared, RequestError> {
    validate_stateless(&request)?;
    if request
        .model
        .as_option()
        .is_none_or(|model| model.trim().is_empty())
    {
        return Err(invalid("model", "需要明确指定模型"));
    }
    let fields =
        serde_json::to_value(&request).map_err(|_| invalid("request", "请求无法序列化"))?;
    for param in ["tools", "additional_tools"] {
        if let Some(tools) = fields.get(param).filter(|v| !v.is_null()) {
            validate_client_tools(tools, param)?;
        }
    }
    if let Some(items) = fields.get("input").and_then(|v| v.as_array()) {
        for item in items {
            if item.get("type").and_then(|v| v.as_str()) == Some("additional_tools") {
                validate_client_tools(
                    item.get("tools")
                        .ok_or_else(|| invalid("input", "additional_tools 缺少工具"))?,
                    "input",
                )?;
            }
        }
    }
    if oauth {
        for param in [
            "max_output_tokens",
            "max_tool_calls",
            "metadata",
            "moderation",
            "multi_agent",
            "prompt",
            "prompt_cache_retention",
            "safety_identifier",
            "temperature",
            "top_logprobs",
            "top_p",
            "truncation",
            "user",
        ] {
            if fields.get(param).is_some() {
                return Err(invalid(param, "当前订阅上游不支持此参数"));
            }
        }
    }
    let downstream_stream = match request.stream {
        O::Missing | O::Null => false,
        O::Value(value) => value,
    };
    let input = match request.input {
        O::Value(Input::Text(text)) => vec![InputItem::Message(Message::Easy(EasyInputMessage {
            content: Content::Text(text),
            role: Role::User,
            phase: O::Missing,
            r#type: None,
            extra: Default::default(),
        }))],
        O::Value(Input::Items(items)) => items,
        _ => return Err(invalid("input", "需要提交完整上下文 input")),
    };
    if oauth && input.iter().any(|item| matches!(item,
        InputItem::Message(Message::Easy(message)) if message.role == Role::System
    ) || matches!(item,
        InputItem::Message(Message::Input(message)) if message.role == llmproxy_core::protocol::responses::request::message::InputRole::System
    )) {
        return Err(invalid("input", "订阅上游不支持 system 消息；请使用 instructions 或 developer 消息"));
    }
    request.input = O::Value(Input::Items(input));
    request.store = O::Value(false);
    request.stream = O::Value(true);
    Ok(Prepared {
        upstream: request,
        downstream_stream,
    })
}

fn validate_client_tools(
    value: &serde_json::Value,
    param: &'static str,
) -> Result<(), RequestError> {
    let tools = value
        .as_array()
        .ok_or_else(|| invalid(param, "工具必须为数组"))?;
    for tool in tools {
        match tool.get("type").and_then(|v| v.as_str()) {
            Some("function" | "custom") => {}
            Some("namespace") => validate_client_tools(
                tool.get("tools")
                    .ok_or_else(|| invalid(param, "namespace 缺少工具"))?,
                param,
            )?,
            _ => {
                return Err(invalid(
                    param,
                    "只支持由客户端执行的 function/custom 工具，禁止托管工具",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_canonical_messages_preserve_fields_and_client_tool_ids() {
        let input = json!([
            {"role":"user","content":"hello"},
            {"type":"function_call","name":"exec_command","call_id":"call_1","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_1","output":"client result"}
        ]);
        let source = json!({"model":"m","input":input,"temperature":0.7,"future":{"keep":true},
            "tools":[{"type":"function","name":"exec_command","parameters":{"type":"object"}}]});
        let prepared =
            prepare_codex_request(serde_json::from_value(source.clone()).unwrap()).unwrap();
        let actual = serde_json::to_value(prepared.upstream).unwrap();
        assert_eq!(
            actual["input"][0],
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]})
        );
        assert_eq!(actual["input"][1], input[1]);
        assert_eq!(actual["input"][2], input[2]);
        assert_eq!(actual["temperature"], source["temperature"]);
        assert_eq!(actual["future"], source["future"]);
        assert_eq!(actual["tools"], source["tools"]);
        assert_eq!(actual["instructions"], "");
        assert_eq!(actual["store"], false);
        assert_eq!(actual["stream"], true);
    }

    #[test]
    fn hosted_tools_are_rejected_even_inside_namespaces() {
        for tool in [
            json!({"type":"web_search"}),
            json!({"type":"namespace","name":"client","tools":[{"type":"code_interpreter"}]}),
        ] {
            let request =
                serde_json::from_value(json!({"model":"m","input":"hello","tools":[tool]}))
                    .unwrap();
            assert_eq!(prepare_oauth_request(request).err().unwrap().param, "tools");
        }
    }

    #[test]
    fn stream_mode_changes_without_losing_multimodal_tool_history() {
        let input = json!([
            {"role":"developer","content":"Follow the tool contract"},
            {"role":"user","content":[
                {"type":"input_image","image_url":"data:image/png;base64,YQ=="},
                {"type":"input_file","file_data":"Yg==","filename":"note.txt"}
            ]},
            {"type":"function_call","name":"lookup","call_id":"call_1","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_1","output":"result"},
            {"type":"reasoning","id":"r1","encrypted_content":"opaque","summary":[]}
        ]);
        for stream in [false, true] {
            let source = json!({"model":"m","input":input,"stream":stream,
                "reasoning":{"effort":"high"},"include":["reasoning.encrypted_content"],
                "tools":[{"type":"namespace","name":"client","tools":[
                    {"type":"function","name":"lookup","parameters":{"type":"object"}}
                ]}],"future_extension":{"keep":true}});
            let prepared =
                prepare_oauth_request(serde_json::from_value(source.clone()).unwrap()).unwrap();
            assert_eq!(prepared.downstream_stream, stream);
            let actual = serde_json::to_value(prepared.upstream).unwrap();
            let mut expected = source;
            expected["store"] = json!(false);
            expected["stream"] = json!(true);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn unsupported_fields_are_rejected_instead_of_dropped() {
        for param in [
            "previous_response_id",
            "conversation",
            "background",
            "max_output_tokens",
            "max_tool_calls",
            "metadata",
            "moderation",
            "multi_agent",
            "prompt",
            "prompt_cache_retention",
            "safety_identifier",
            "temperature",
            "top_logprobs",
            "top_p",
            "truncation",
            "user",
        ] {
            let mut source = json!({"model":"m","input":"hello"});
            source[param] = serde_json::Value::Null;
            let error = prepare_oauth_request(serde_json::from_value(source).unwrap())
                .err()
                .unwrap();
            assert_eq!(error.param, param);
        }
        for source in [
            json!({"model":"m","input":"hello","store":true}),
            json!({"model":"m"}),
            json!({"model":"","input":"hello"}),
            json!({"model":"m","input":[{"type":"message","role":"system","content":"hello"}]}),
        ] {
            assert!(prepare_oauth_request(serde_json::from_value(source).unwrap()).is_err());
        }
    }

    #[test]
    fn plain_text_becomes_a_user_item_and_defaults_to_nonstream() {
        let prepared = prepare_oauth_request(
            serde_json::from_value(json!({"model":"m","input":"hello"})).unwrap(),
        )
        .unwrap();
        assert!(!prepared.downstream_stream);
        assert_eq!(
            serde_json::to_value(prepared.upstream).unwrap(),
            json!({
                "model":"m","input":[{"role":"user","content":"hello"}],"store":false,"stream":true
            })
        );
    }
}
