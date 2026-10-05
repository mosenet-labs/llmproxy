//! 独立书写四协议线缆夹具，避免用被测转换器生成其自身的测试输入。
#![allow(dead_code)] // 各验收二进制仅使用一部分公共夹具，未使用的部分仍由其他套件覆盖。

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

/// 以简单但有效的 Schema 检查约束通过 IR 后仍进入目标协议字段。
/// 参考：https://developers.openai.com/api/docs/guides/structured-outputs
/// 参考：https://ai.google.dev/gemini-api/docs/generate-content/structured-output
pub fn structured_request(protocol: Protocol, model: &str) -> Value {
    let mut body = request(
        protocol,
        model,
        "Return a JSON object with answer set to OK. No other fields.",
    );
    let schema = json!({"type":"object","properties":{"answer":{"type":"string","enum":["OK"]}},"required":["answer"],"additionalProperties":false});
    match protocol {
        Protocol::OpenAiChat => {
            body["response_format"] = json!({"type":"json_schema","json_schema":{"name":"answer","schema":schema,"strict":true}})
        }
        Protocol::OpenAiResponses => {
            body["text"] = json!({"format":{"type":"json_schema","name":"answer","schema":schema,"strict":true}})
        }
        Protocol::AnthropicMessages => {
            body["output_config"] = json!({"format":{"type":"json_schema","schema":schema}})
        }
        Protocol::Gemini => {
            body["generationConfig"]["responseMimeType"] = json!("application/json");
            body["generationConfig"]["responseJsonSchema"] = schema;
        }
    }
    body
}

/// 合成的公开测试样本，无用户文件；HTTP 与真实模型验收共用。
#[derive(Clone, Copy, Debug)]
pub enum MediaCase {
    Image,
    Pdf,
    Audio,
    Video,
}

impl MediaCase {
    /// 只枚举来源协议有原生载体的输入，避免伪造并不存在的协议能力。
    pub fn supported(self, protocol: Protocol) -> bool {
        match self {
            Self::Image | Self::Pdf => true,
            Self::Audio => matches!(protocol, Protocol::OpenAiChat | Protocol::Gemini),
            Self::Video => protocol == Protocol::Gemini,
        }
    }
}

/// 图片为纯蓝色 PNG，PDF 写有 HELLO，音频朗读 Hello，视频为纯蓝色画面。
/// 参考：https://developers.openai.com/api/docs/guides/file-inputs
/// 参考：https://developers.openai.com/api/docs/guides/audio-chat-completions
/// 参考：https://ai.google.dev/gemini-api/docs/generate-content/audio
pub fn media_request(protocol: Protocol, model: &str, case: MediaCase) -> Value {
    use base64::{Engine, engine::general_purpose::STANDARD};
    assert!(case.supported(protocol));
    let (mime, bytes, prompt): (&str, &[u8], &str) = match case {
        MediaCase::Image => (
            "image/png",
            include_bytes!("assets/blue.png"),
            "What is the dominant color of this image? Reply with one color name.",
        ),
        MediaCase::Pdf => (
            "application/pdf",
            include_bytes!("assets/hello.pdf"),
            "Read the word printed in this PDF. Reply with that word only.",
        ),
        MediaCase::Audio => (
            "audio/wav",
            include_bytes!("assets/hello.wav"),
            "Transcribe the single word spoken in this audio. Reply with that word only.",
        ),
        MediaCase::Video => (
            "video/mp4",
            include_bytes!("assets/blue.mp4"),
            "What is the dominant color of this video? Reply with one color name.",
        ),
    };
    let data = STANDARD.encode(bytes);
    let url = format!("data:{mime};base64,{data}");
    let mut body = request(protocol, model, prompt);
    match protocol {
        Protocol::OpenAiChat => {
            let media = match case {
                MediaCase::Image => json!({"type":"image_url","image_url":{"url":url}}),
                MediaCase::Pdf => {
                    json!({"type":"file","file":{"file_data":url,"filename":"hello.pdf"}})
                }
                MediaCase::Audio => {
                    json!({"type":"input_audio","input_audio":{"data":data,"format":"wav"}})
                }
                MediaCase::Video => unreachable!(),
            };
            body["messages"][0]["content"] = json!([{"type":"text","text":prompt},media]);
        }
        Protocol::OpenAiResponses => {
            let media = match case {
                MediaCase::Image => json!({"type":"input_image","image_url":url}),
                MediaCase::Pdf => {
                    json!({"type":"input_file","file_data":url,"filename":"hello.pdf"})
                }
                _ => unreachable!(),
            };
            body["input"][0]["content"] = json!([{"type":"input_text","text":prompt},media]);
        }
        Protocol::AnthropicMessages => {
            let kind = if matches!(case, MediaCase::Image) {
                "image"
            } else {
                "document"
            };
            body["messages"][0]["content"] = json!([{"type":"text","text":prompt},{"type":kind,"source":{"type":"base64","media_type":mime,"data":data}}]);
        }
        Protocol::Gemini => {
            body["contents"][0]["parts"] =
                json!([{"text":prompt},{"inlineData":{"mimeType":mime,"data":data}}])
        }
    }
    body
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
    let mut body = request(
        protocol,
        model,
        "Call lookup with q set to test. After receiving its result, reply with exactly OK.",
    );
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

/// 新建代码执行环境的请求，避免携带来源 Provider 的容器和文件 ID。
/// 参考：https://ai.google.dev/api/generate-content#Tool
pub fn code_request(protocol: Protocol, model: &str) -> Value {
    let mut body = request(
        protocol,
        model,
        "Use the code execution tool to run Python print(37 * 29). Report the result.",
    );
    match protocol {
        Protocol::OpenAiResponses => {
            body["max_output_tokens"] = json!(1024);
            body["tools"] = json!([{"type":"code_interpreter","container":{"type":"auto"}}]);
        }
        Protocol::AnthropicMessages => {
            body["max_tokens"] = json!(1024);
            body["tools"] = json!([{"type":"code_execution_20250522","name":"code_execution"}]);
        }
        Protocol::Gemini => {
            body["generationConfig"]["maxOutputTokens"] = json!(1024);
            body["tools"] = json!([{"codeExecution":{}}]);
        }
        Protocol::OpenAiChat => panic!("Chat 没有原生代码执行声明"),
    }
    body
}

/// 使用各协议原生努力等级表达同一配置；实际是否生效由目标模型验收。
/// 参考：https://ai.google.dev/api/generate-content#ThinkingConfig
pub fn reasoning_request(protocol: Protocol, model: &str) -> Value {
    let mut body = request(protocol, model, "What is 37 * 29? Reply with exactly 1073.");
    match protocol {
        Protocol::OpenAiChat => {
            body["max_completion_tokens"] = json!(1024);
            body["reasoning_effort"] = json!("low");
        }
        Protocol::OpenAiResponses => {
            body["max_output_tokens"] = json!(1024);
            body["reasoning"] = json!({"effort":"low","summary":"auto"});
        }
        Protocol::AnthropicMessages => {
            body["max_tokens"] = json!(1024);
            body["thinking"] = json!({"type":"adaptive"});
            body["output_config"] = json!({"effort":"low"});
        }
        Protocol::Gemini => {
            body["generationConfig"] = json!({"maxOutputTokens":1024,"thinkingConfig":{"thinkingLevel":"low","includeThoughts":true}});
        }
    }
    body
}

/// 明确的数字预算只用于具有原生预算字段的协议，不能用努力等级冒充。
/// 参考：https://ai.google.dev/gemini-api/docs/thinking#thinking-budgets
/// 参考：https://platform.claude.com/docs/en/build-with-claude/extended-thinking
pub fn reasoning_budget_request(protocol: Protocol, model: &str) -> Value {
    let mut body = request(protocol, model, "What is 37 * 29? Reply with exactly 1073.");
    match protocol {
        Protocol::AnthropicMessages => {
            body["max_tokens"] = json!(2048);
            body["thinking"] = json!({"type":"enabled","budget_tokens":1024});
        }
        Protocol::Gemini => {
            body["generationConfig"] = json!({"maxOutputTokens":2048,"thinkingConfig":{"thinkingBudget":1024,"includeThoughts":true}});
        }
        _ => panic!("该协议没有原生数字推理预算"),
    }
    body
}

/// 独立原生输出夹具，执行记录与客户端函数调用严格区分。
pub fn code_response(protocol: Protocol) -> Value {
    let mut body = response(protocol, false);
    match protocol {
        Protocol::OpenAiResponses => {
            body["output"] = json!([{"type":"code_interpreter_call","id":"server-private","container_id":"private-container","status":"completed","code":"print(1073)","outputs":[{"type":"logs","logs":"1073"}]}])
        }
        Protocol::AnthropicMessages => {
            body["content"] = json!([{"type":"server_tool_use","id":"server-private","name":"code_execution","input":{"code":"print(1073)"}},{"type":"code_execution_tool_result","tool_use_id":"server-private","content":{"type":"code_execution_result","stdout":"1073","stderr":"","return_code":0,"content":[]}}])
        }
        Protocol::Gemini => {
            body["candidates"][0]["content"]["parts"] = json!([{"executableCode":{"language":"PYTHON","code":"print(1073)"}},{"codeExecutionResult":{"outcome":"OUTCOME_OK","output":"1073"}}])
        }
        Protocol::OpenAiChat => panic!("Chat 没有原生代码执行输出"),
    }
    body
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
