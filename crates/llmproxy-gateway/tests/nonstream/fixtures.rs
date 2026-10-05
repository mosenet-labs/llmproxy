//! 独立书写四协议线缆夹具，避免用被测转换器生成其自身的测试输入。
use llmproxy_core::{
    adapter::protocol_codec::{ProtocolCodec, RequestTarget},
    ir,
    protocol::{Protocol, Request, Response},
};
use serde_json::{Value, json};

/// 使用短输出上限，真实联调时限制 token 消耗。
pub fn request(protocol: Protocol, model: &str, prompt: &str) -> Value {
    match protocol {
        Protocol::OpenAiChat => {
            json!({"model":model,"messages":[{"role":"user","content":prompt}],"max_completion_tokens":128,"stream":false})
        }
        Protocol::OpenAiResponses => {
            json!({"model":model,"input":[{"role":"user","content":prompt}],"max_output_tokens":128,"stream":false})
        }
        Protocol::AnthropicMessages => {
            json!({"model":model,"messages":[{"role":"user","content":prompt}],"max_tokens":128,"stream":false})
        }
        Protocol::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":prompt}]}],"generationConfig":{"maxOutputTokens":128}})
        }
    }
}

/// 将模型输出作为下一轮历史，加入对应的客户端工具结果；仍经协议 struct 和 IR 编码。
pub fn tool_result_request(
    protocol: Protocol,
    model: &str,
    initial: &Value,
    response: &ir::response::Response,
    call: &ir::message::ToolCall,
) -> Result<Value, &'static str> {
    use ir::{message::ToolResult, request::Item as Input, response::Item as Output};
    let mut request =
        decode_request(protocol, &serde_json::to_vec(initial).unwrap()).without_source();
    for item in &response.items {
        request.items.push(match item {
            Output::Message(index) => {
                let index = *index;
                let position = request.messages.len();
                request
                    .messages
                    .push(response.messages[index].clone().into());
                Input::Message(position)
            }
            Output::ToolCall { call, item_id } => Input::ToolCall {
                call: call.clone(),
                item_id: item_id.clone(),
            },
            Output::Reasoning(text) => Input::Reasoning(text.clone()),
            Output::Opaque(_) | Output::ServerOutput(_) => {
                return Err("工具回合包含不可移植的专属输出");
            }
        });
    }
    request.items.push(Input::ToolResult(ToolResult {
        id: call.id.clone(),
        name: Some(call.name.clone()),
        content: json!("OK"),
    }));
    // 第二轮只验证工具结果和历史的转换，禁止模型再次调用工具。
    request.generation.tool_choice = Some(ir::request::controls::ToolChoice::None);
    let encoded = protocol
        .encode_request_for(
            &request,
            &RequestTarget {
                model,
                max_output_tokens: None,
            },
        )
        .map_err(|_| "工具结果历史无法编码为客户端协议")?
        .body;
    let value = match encoded {
        Request::Chat(body) => serde_json::to_value(body),
        Request::Responses(body) => serde_json::to_value(body),
        Request::Messages(body) => serde_json::to_value(body),
        Request::Gemini(body) => serde_json::to_value(body),
    };
    value.map_err(|_| "工具结果请求序列化失败")
}

/// 四种客户端协议分别声明并强制调用同一函数，验证真实工具入口和参数转换。
pub fn tool_request(protocol: Protocol, model: &str) -> Value {
    let mut body = request(protocol, model, "Call lookup with q set to test.");
    let schema = json!({"type":"object","properties":{"q":{"type":"string"}},"required":["q"]});
    match protocol {
        Protocol::OpenAiChat => {
            body["tools"] = json!([{"type":"function","function":{"name":"lookup","description":"Look up the given value","parameters":schema}}]);
            body["tool_choice"] = json!({"type":"function","function":{"name":"lookup"}});
        }
        Protocol::OpenAiResponses => {
            body["tools"] = json!([{"type":"function","name":"lookup","description":"Look up the given value","parameters":schema}]);
            body["tool_choice"] = json!({"type":"function","name":"lookup"});
        }
        Protocol::AnthropicMessages => {
            body["tools"] = json!([{"name":"lookup","description":"Look up the given value","input_schema":schema}]);
            body["tool_choice"] = json!({"type":"tool","name":"lookup"});
        }
        Protocol::Gemini => {
            body["tools"] = json!([{"functionDeclarations":[{"name":"lookup","description":"Look up the given value","parametersJsonSchema":schema}]}]);
            body["toolConfig"] =
                json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["lookup"]}});
        }
    }
    body
}

/// 各协议独立报告同一口径：总输入 10，其中缓存 4；输出 3。
pub fn response(protocol: Protocol, tool: bool) -> Value {
    match protocol {
        Protocol::OpenAiChat => {
            json!({"id":"mock","model":"upstream-model","object":"chat.completion","created":1,
            "choices":[{"index":0,"finish_reason":if tool {"tool_calls"} else {"stop"},"message":if tool {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"test\"}"}}]})
            } else {json!({"role":"assistant","content":"OK"})}}],
            "usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":4}}})
        }
        Protocol::OpenAiResponses => {
            json!({"id":"mock","model":"upstream-model","object":"response","created_at":1,"status":"completed",
            "output":if tool {json!([{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{\"q\":\"test\"}","status":"completed"}])}
            else {json!([{"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"OK","annotations":[]}]}])},
            "usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}}})
        }
        Protocol::AnthropicMessages => {
            json!({"type":"message","id":"mock","model":"upstream-model","role":"assistant","stop_reason":if tool {"tool_use"} else {"end_turn"},
            "content":if tool {json!([{"type":"tool_use","id":"call_1","name":"lookup","input":{"q":"test"}}])} else {json!([{"type":"text","text":"OK"}])},
            "usage":{"input_tokens":6,"cache_read_input_tokens":4,"output_tokens":3}})
        }
        Protocol::Gemini => {
            json!({"responseId":"mock","modelVersion":"upstream-model","candidates":[{"index":0,"finishReason":"STOP","content":{"role":"model",
            "parts":if tool {json!([{"functionCall":{"id":"call_1","name":"lookup","args":{"q":"test"}}}])} else {json!([{"text":"OK"}])}}}],
            "usageMetadata":{"promptTokenCount":10,"cachedContentTokenCount":4,"candidatesTokenCount":3,"totalTokenCount":13}})
        }
    }
}

/// 在 HTTP 边界直接解析协议 struct，再进入公共 IR。
pub fn decode_request(protocol: Protocol, bytes: &[u8]) -> ir::request::Request {
    let raw = match protocol {
        Protocol::OpenAiChat => Request::Chat(serde_json::from_slice(bytes).unwrap()),
        Protocol::OpenAiResponses => Request::Responses(serde_json::from_slice(bytes).unwrap()),
        Protocol::AnthropicMessages => Request::Messages(serde_json::from_slice(bytes).unwrap()),
        Protocol::Gemini => Request::Gemini(serde_json::from_slice(bytes).unwrap()),
    };
    protocol.decode_request(&raw).unwrap()
}

/// 真实响应解析失败时只报告阶段，不输出 Provider 正文。
pub fn decode_response(
    protocol: Protocol,
    bytes: &[u8],
) -> Result<ir::response::Response, &'static str> {
    let raw = match protocol {
        Protocol::OpenAiChat => serde_json::from_slice(bytes).map(Response::Chat),
        Protocol::OpenAiResponses => serde_json::from_slice(bytes).map(Response::Responses),
        Protocol::AnthropicMessages => serde_json::from_slice(bytes).map(Response::Messages),
        Protocol::Gemini => serde_json::from_slice(bytes).map(Response::Gemini),
    }
    .map_err(|_| "响应不符合客户端协议结构")?;
    protocol
        .decode_response(&raw)
        .map_err(|_| "响应无法解码到 IR")
}
