//! 独立来源 SSE 夹具；测试 JSON 属于 HTTP 边界，不复用目标编码器生成来源。
use llmproxy_core::{
    adapter::protocol_codec::ProtocolCodec,
    ir::stream::{Event, Head, Key, Limits},
    protocol::{
        Protocol,
        stream::{self, sse},
    },
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn provider(
    protocol: Protocol,
    port: u16,
    read_timeout_ms: u64,
) -> llmproxy_store::ProviderInput {
    llmproxy_store::ProviderInput {
        name: protocol.as_str().into(),
        paths: llmproxy_store::ProviderPaths::single(protocol),
        host: "127.0.0.1".into(),
        port,
        tls: false,
        api_key: "stream-key".into(),
        enabled: true,
        models_path: "/models".into(),
        models_protocol: protocol,
        anthropic_version: Some("2023-06-01".into()),
        messages_auth: llmproxy_core::protocol::MessagesAuth::ApiKey,
        connect_timeout_ms: 2000,
        read_timeout_ms,
        write_timeout_ms: 1000,
    }
}

pub fn path(protocol: Protocol, model: &str) -> String {
    if protocol == Protocol::Gemini {
        format!("/v1beta/models/{model}:streamGenerateContent?alt=sse")
    } else {
        super::nonstream::path(protocol, model)
    }
}

/// 第一组含可见文字，最后一组才结束；工具回合包含完整函数与缓存计数。
pub fn frames(protocol: Protocol, tool: bool) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut start = Vec::new();
    let mut end = Vec::new();
    match protocol {
        Protocol::OpenAiChat => {
            let chunk = |choices| json!({"id":"raw","model":"upstream-model","created":1,"object":"chat.completion.chunk","choices":choices});
            start.push(chunk(
                json!([{"index":0,"delta":{"role":"assistant","content":"你"}}]),
            ));
            start.push(chunk(json!([{"index":0,"delta":{"content":"好"}}])));
            if tool {
                end.push(chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"lookup","arguments":"{\"city\":\"Paris\"}"}}]}}])));
            }
            end.push(chunk(json!([{"index":0,"delta":{},"finish_reason":if tool {"tool_calls"} else {"stop"}}])));
            let mut usage = chunk(json!([]));
            usage["usage"] = json!({"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":4}});
            end.push(usage);
        }
        Protocol::AnthropicMessages => {
            start.extend([
                json!({"type":"message_start","message":{"id":"raw","type":"message","model":"upstream-model","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":6,"cache_read_input_tokens":4,"output_tokens":0}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你"}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好"}}),
            ]);
            end.push(json!({"type":"content_block_stop","index":0}));
            if tool {
                end.extend([
                json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"lookup","input":{}}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\"Paris\"}"}}),
                json!({"type":"content_block_stop","index":1}),
            ]);
            }
            end.extend([json!({"type":"message_delta","delta":{"stop_reason":if tool {"tool_use"} else {"end_turn"}},"usage":{"output_tokens":3}}), json!({"type":"message_stop"})]);
        }
        Protocol::OpenAiResponses => {
            let response = |output| json!({"id":"raw","model":"upstream-model","created_at":1,"object":"response","output":output});
            start.extend([
                json!({"type":"response.created","sequence_number":0,"response":response(json!([]))}),
                json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":"m","type":"message","role":"assistant","status":"in_progress","content":[]}}),
                json!({"type":"response.content_part.added","sequence_number":2,"output_index":0,"content_index":0,"item_id":"m","part":{"type":"output_text","text":"","annotations":[]}}),
                json!({"type":"response.output_text.delta","sequence_number":3,"output_index":0,"content_index":0,"item_id":"m","delta":"你"}),
                json!({"type":"response.output_text.delta","sequence_number":4,"output_index":0,"content_index":0,"item_id":"m","delta":"好"}),
            ]);
            let message = json!({"id":"m","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"你好","annotations":[]}]});
            end.push(json!({"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":message}));
            let mut output = vec![message];
            if tool {
                let call = json!({"id":"fc","type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"city\":\"Paris\"}"});
                end.extend([json!({"type":"response.output_item.added","sequence_number":6,"output_index":1,"item":{"id":"fc","type":"function_call","call_id":"call_1","name":"lookup","arguments":""}}),json!({"type":"response.output_item.done","sequence_number":7,"output_index":1,"item":call})]);
                output.push(call);
            }
            let mut completed = response(json!(output));
            completed["status"] = json!("completed");
            completed["usage"] = json!({"input_tokens":10,"output_tokens":3,"total_tokens":13,"input_tokens_details":{"cached_tokens":4}});
            end.push(json!({"type":"response.completed","sequence_number":8,"response":completed}));
        }
        Protocol::Gemini => {
            let response = |parts| json!({"responseId":"raw","modelVersion":"upstream-model","candidates":[{"index":0,"content":{"role":"model","parts":parts}}]});
            start.extend([
                response(json!([{"text":"你"}])),
                response(json!([{"text":"好"}])),
            ]);
            if tool {
                end.push(response(json!([{"functionCall":{"id":"call_1","name":"lookup","args":{"city":"Paris"}},"thoughtSignature":"private-signature"}])));
            }
            end.push(json!({"responseId":"raw","modelVersion":"upstream-model","candidates":[{"index":0,"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3,"totalTokenCount":13,"cachedContentTokenCount":4}}));
        }
    }
    let encode = |value: Value| {
        let name = value.get("type").and_then(Value::as_str);
        let mut bytes = name
            .map(|name| format!("event: {name}\r\n"))
            .unwrap_or_default()
            .into_bytes();
        bytes.extend_from_slice(b"data: ");
        bytes.extend(serde_json::to_vec(&value).unwrap());
        bytes.extend_from_slice(b"\r\n\r\n");
        bytes
    };
    let start = start.into_iter().map(encode).collect();
    let mut end: Vec<_> = end.into_iter().map(encode).collect();
    if protocol == Protocol::OpenAiChat {
        end.push(b"data: [DONE]\r\n\r\n".to_vec());
    }
    (start, end)
}

/// 客户端重新解码目标事件，检查实际文本、参数、结束状态及累计用量。
pub struct Observed {
    pub decoder: llmproxy_core::adapter::protocol_codec::stream::Decoder,
    pub text: String,
    pub candidates: BTreeMap<u64, String>,
    pub tools: BTreeMap<Key, (Option<String>, String, String)>,
    framing: sse::Decoder,
}
impl Observed {
    pub fn new(protocol: Protocol) -> Self {
        Self {
            decoder: protocol.stream_decoder(Limits::default()),
            text: String::new(),
            candidates: BTreeMap::new(),
            tools: BTreeMap::new(),
            framing: sse::Decoder::new(8 * 1024 * 1024),
        }
    }
    pub fn push(&mut self, protocol: Protocol, bytes: &[u8]) {
        for byte in bytes {
            if let Some(frame) = self.framing.push(*byte).unwrap() {
                let raw = sse::decode(protocol, &frame).unwrap();
                self.raw(&raw);
            }
        }
    }
    pub fn finish(&mut self, protocol: Protocol) {
        self.framing.finish().unwrap();
        self.raw(&stream::Event::End(protocol));
    }
    fn raw(&mut self, raw: &stream::Event) {
        for event in self.decoder.push(raw).unwrap() {
            match event {
                Event::TextDelta { key, text } => {
                    self.text.push_str(&text);
                    self.candidates
                        .entry(key.candidate)
                        .or_default()
                        .push_str(&text);
                }
                Event::PartStart {
                    key,
                    head: Head::Tool(head),
                } => {
                    self.tools
                        .insert(key, (head.id, head.name.unwrap_or_default(), String::new()));
                }
                Event::ToolDelta {
                    key,
                    id,
                    name,
                    arguments,
                } => {
                    let call = self.tools.get_mut(&key).unwrap();
                    if id.is_some() {
                        call.0 = id;
                    }
                    if let Some(name) = name {
                        call.1.push_str(&name);
                    }
                    if let Some(args) = arguments {
                        call.2.push_str(&args);
                    }
                }
                Event::Failure(_) => panic!("不能将失败当作成功"),
                _ => {}
            }
        }
    }
}

/// 等待请求收尾日志；每轮只有一个完成事件，用量快照关联到同一请求 ID。
pub fn assert_logs(gateway: &super::support::Gateway, expected: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let logs: Vec<Value> = gateway
            .logs()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let requests: Vec<_> = logs
            .iter()
            .filter(|line| {
                line["fields"]["event_kind"] == "request"
                    && super::nonstream::ALL
                        .iter()
                        .any(|protocol| line["fields"]["protocol"] == protocol.as_str())
            })
            .collect();
        if requests.len() < expected && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
            continue;
        }
        assert_eq!(requests.len(), expected, "每次请求只完成一次统计");
        let ids: std::collections::BTreeSet<_> = requests
            .iter()
            .map(|line| {
                assert!(
                    line["fields"]["upstream"].is_string(),
                    "取消不能丢失已选路由"
                );
                line["fields"]["request_id"].as_u64().unwrap()
            })
            .collect();
        assert_eq!(ids.len(), expected);
        let mut observed = std::collections::BTreeSet::new();
        for usage in logs
            .iter()
            .filter(|line| line["fields"]["event_kind"] == "usage")
        {
            let id = usage["span"]["request_id"]
                .as_u64()
                .or_else(|| {
                    usage["spans"]
                        .as_array()?
                        .iter()
                        .rev()
                        .find_map(|span| span["request_id"].as_u64())
                })
                .expect("用量必须在原请求 span 中");
            assert!(ids.contains(&id), "用量与最终请求日志必须关联");
            assert!(observed.insert(id), "累计用量不能重复记录");
        }
        break;
    }
}
