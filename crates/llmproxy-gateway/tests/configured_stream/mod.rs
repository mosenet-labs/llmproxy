//! 真实流式验收只读配置库，复用父模块配置；临时路由不改变开发配置。
use super::{configuration, live_database, nonstream, support};
use llmproxy_core::{
    adapter::protocol_codec::ProtocolCodec,
    ir::{
        self,
        stream::{Event, Head, Key, Limits},
    },
    protocol::{
        Protocol,
        stream::{self, sse},
    },
};
use std::collections::BTreeMap;

struct Reply {
    text: String,
    calls: BTreeMap<Key, ir::message::ToolCall>,
    usage: ir::usage::Usage,
    gemini: Vec<llmproxy_core::protocol::gemini::request::message::Part>,
    first_before_end: bool,
}

/// 逐块解码真实目标事件，保存客户端可回传的函数及同协议 Gemini 原生签名。
async fn read(protocol: Protocol, mut response: reqwest::Response) -> Result<Reply, String> {
    if !response.status().is_success() {
        return Err(super::provider_failure(response).await);
    }
    let mut framing = sse::Decoder::new(8 * 1024 * 1024);
    let mut decoder = protocol.stream_decoder(Limits::default());
    let mut text = String::new();
    let mut calls: BTreeMap<Key, ir::message::ToolCall> = BTreeMap::new();
    let mut gemini = Vec::new();
    let mut first_before_end = false;
    while let Some(chunk) = response.chunk().await.map_err(|_| "读取 HTTP 流失败")? {
        for byte in chunk {
            if let Some(frame) = framing.push(byte).map_err(|_| "SSE 分帧失败")? {
                let raw = sse::decode(protocol, &frame).map_err(|_| "协议事件解析失败")?;
                if let stream::Event::Gemini(response) = &raw {
                    for candidate in response.candidates.as_option().into_iter().flatten() {
                        if let Some(content) = candidate.content.as_option() {
                            gemini.extend(
                                content
                                    .parts
                                    .iter()
                                    .filter(|part| part.function_call.as_option().is_some())
                                    .cloned(),
                            );
                        }
                    }
                }
                for event in decoder.push(&raw).map_err(|_| "事件无法解码到 IR")? {
                    match event {
                        Event::TextDelta { text: delta, .. } => {
                            if text.is_empty() {
                                first_before_end = decoder.state().ended().is_none();
                            }
                            text.push_str(&delta);
                        }
                        Event::PartStart {
                            key,
                            head: Head::Tool(head),
                        } => {
                            calls.insert(
                                key,
                                ir::message::ToolCall {
                                    id: head.id,
                                    name: head.name.unwrap_or_default(),
                                    arguments: serde_json::Value::String(String::new()),
                                },
                            );
                        }
                        Event::ToolDelta {
                            key,
                            id,
                            name,
                            arguments,
                        } => {
                            let call = calls.get_mut(&key).ok_or("缺少工具调用起始事件")?;
                            if id.is_some() {
                                call.id = id;
                            }
                            if let Some(name) = name {
                                call.name.push_str(&name);
                            }
                            if let Some(arguments) = arguments {
                                call.arguments.as_str().ok_or("工具参数类型不一致")?;
                                call.arguments = serde_json::Value::String(format!(
                                    "{}{}",
                                    call.arguments.as_str().unwrap(),
                                    arguments
                                ));
                            }
                        }
                        Event::Failure(_) => return Err("Provider 流内生成失败".into()),
                        _ => {}
                    }
                }
            }
        }
    }
    framing.finish().map_err(|_| "SSE 流截断")?;
    decoder
        .push(&stream::Event::End(protocol))
        .map_err(|_| "协议未正常结束")?;
    let usage = decoder
        .state()
        .usage()
        .snapshot()
        .cloned()
        .ok_or("未报告用量")?;
    if usage.input_tokens.is_none() || usage.output_tokens.is_none() {
        return Err("缺少输入输出计数".into());
    }
    Ok(Reply {
        text,
        calls,
        usage,
        gemini,
        first_before_end,
    })
}

/// 流式意图使用各协议原生入口；工具自动选择及推理设置沿用非流式真实验收条件。
fn request(source: Protocol, target: Protocol, alias: &str, tool: bool) -> serde_json::Value {
    use serde_json::json;
    let mut body = if tool {
        nonstream::fixtures::tool_request(source, alias)
    } else {
        nonstream::fixtures::request(source, alias, "Reply with exactly OK.")
    };
    if tool {
        match source {
            Protocol::OpenAiChat => {
                body["tool_choice"] = json!("auto");
                if target != Protocol::Gemini {
                    body["reasoning_effort"] = json!("none");
                }
            }
            Protocol::OpenAiResponses => {
                body["tool_choice"] = json!("auto");
                if target != Protocol::Gemini {
                    body["reasoning"] = json!({"effort":"none"});
                }
            }
            Protocol::AnthropicMessages => {
                body["tool_choice"] = json!({"type":"auto"});
                if target != Protocol::Gemini {
                    body["thinking"] = json!({"type":"disabled"});
                }
            }
            Protocol::Gemini => {
                body["toolConfig"] = json!({"functionCallingConfig":{"mode":"AUTO"}});
                if target != Protocol::Gemini {
                    body["generationConfig"]["thinkingConfig"] = json!({"thinkingBudget":0});
                }
            }
        }
    }
    if source != Protocol::Gemini {
        body["stream"] = json!(true);
    }
    if source == Protocol::OpenAiChat {
        body["stream_options"] = json!({"include_usage":true});
    }
    body
}

/// Gemini 流式模式由 URL 指定，其他协议继续使用原有端点。
fn path(protocol: Protocol, alias: &str) -> String {
    if protocol == Protocol::Gemini {
        format!("/v1beta/models/{alias}:streamGenerateContent?alt=sse")
    } else {
        nonstream::path(protocol, alias)
    }
}

#[tokio::test]
#[ignore = "真实四协议流式与工具双回合，显式运行会产生少量 token 费用"]
async fn configured_provider_stream_matrix() {
    let config = configuration();
    let database = live_database(&config).await;
    let master_key = config.get("LLMPROXY_MASTER_KEY").expect("主密钥已配置");
    let gateway = support::Gateway::database(&database.url, master_key);
    let continuation = support::Gateway::database(&database.url, master_key);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap();
    let mut failures = Vec::new();
    for target in nonstream::ALL {
        for source in nonstream::ALL {
            if config
                .get("LLMPROXY_LIVE_PAIR")
                .is_some_and(|pair| pair != &format!("{}:{}", source.as_str(), target.as_str()))
            {
                continue;
            }
            let alias = nonstream::alias(source, target);
            for tool in [false, true] {
                let result = async {
                    let input = request(source, target, &alias, tool);
                    let response = client
                        .post(format!(
                            "http://{}{}",
                            gateway.address,
                            path(source, &alias)
                        ))
                        .bearer_auth(&gateway.api_key)
                        .json(&input)
                        .send()
                        .await
                        .map_err(|_| "HTTP 连接失败")?;
                    let reply = read(source, response).await?;
                    if tool {
                        if reply.calls.len() != 1 {
                            return Err("未收到唯一函数调用".into());
                        }
                        let call = reply.calls.values().next().unwrap();
                        let args: serde_json::Value =
                            serde_json::from_str(call.arguments.as_str().unwrap())
                                .map_err(|_| "函数参数 JSON 无效")?;
                        if call.name != "lookup" || args["q"] != "test" {
                            return Err("函数名称或参数不一致".into());
                        }
                        let mut output = ir::response::Response::new(source);
                        output.items.push(ir::response::Item::ToolCall {
                            call: call.clone(),
                            item_id: None,
                        });
                        let mut next = nonstream::fixtures::tool_result_request(
                            source, &alias, &input, &output, call,
                        )?;
                        // 同 Gemini 客户端负责回传原生 Part；跨协议引用由另一实例从数据库恢复。
                        if source == Protocol::Gemini {
                            for content in next["contents"].as_array_mut().unwrap() {
                                for part in content["parts"].as_array_mut().unwrap() {
                                    if part.get("functionCall").is_some() {
                                        let original = reply
                                            .gemini
                                            .iter()
                                            .find(|part| {
                                                part.function_call.as_option().is_some_and(
                                                    |original| original.name == call.name,
                                                )
                                            })
                                            .ok_or("Gemini 原生调用丢失")?;
                                        *part = serde_json::to_value(original)
                                            .map_err(|_| "Gemini 调用序列化失败")?;
                                    }
                                }
                            }
                        }
                        let response = client
                            .post(format!(
                                "http://{}{}",
                                continuation.address,
                                path(source, &alias)
                            ))
                            .bearer_auth(&continuation.api_key)
                            .json(&next)
                            .send()
                            .await
                            .map_err(|_| "工具回传连接失败")?;
                        let followup = read(source, response).await?;
                        if !followup.text.contains("OK") {
                            return Err("工具结果没有进入最终回答".into());
                        }
                    } else if !reply.text.contains("OK") || !reply.first_before_end {
                        return Err("未收到正常文本增量".into());
                    }
                    println!(
                        "PASS {} {} -> {} input={:?} output={:?} cache_read={:?}",
                        if tool { "tool" } else { "text" },
                        source.as_str(),
                        target.as_str(),
                        reply.usage.input_tokens,
                        reply.usage.output_tokens,
                        reply.usage.cache.read_input_tokens
                    );
                    Ok::<_, String>(())
                }
                .await;
                if let Err(reason) = result {
                    failures.push(format!(
                        "{} {} -> {}: {reason}",
                        if tool { "tool" } else { "text" },
                        source.as_str(),
                        target.as_str()
                    ));
                }
            }
        }
    }
    drop(continuation);
    drop(gateway);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
