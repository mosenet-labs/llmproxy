//! 配置库只读检查；真实联调必须显式运行 ignored 测试。

#[allow(dead_code)] // 模拟验收还会使用该目录中的夹具和测试密钥。
mod nonstream;
mod support;

use std::collections::HashMap;

use llmproxy_store::{ProviderInput, ProviderStore};

/// 从工作区配置读取测试库连接，避免把凭据写入测试输出或修改进程环境。
fn configuration() -> HashMap<String, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
    let mut values: HashMap<_, _> = dotenvy::from_path_iter(path)
        .expect("读取工作区 .env")
        .map(|entry| entry.expect("解析配置项"))
        .collect();
    values.extend(std::env::vars());
    values
}

/// 默认只读测试库；使用开发库时必须显式指定配置键。
fn database_url(config: &HashMap<String, String>) -> &str {
    let key = config
        .get("LLMPROXY_LIVE_DATABASE_ENV")
        .map(String::as_str)
        .unwrap_or("LLMPROXY_TEST_DATABASE_URL");
    assert!(matches!(
        key,
        "LLMPROXY_TEST_DATABASE_URL" | "LLMPROXY_DATABASE_URL"
    ));
    config.get(key).expect("联调数据库已配置")
}

/// 使用模拟 Provider 向已配置 OpenObserve 写入测试遥测，再读取实际存储结果。
/// 不调用真实模型，不修改 PostgreSQL；认证信息不进入测试输出。
/// 参考：https://openobserve.ai/docs/reference/api/search/search/
#[tokio::test]
#[ignore = "需要本地 OpenObserve；显式写入少量验收日志与遥测"]
async fn configured_openobserve_nonstream() {
    use llmproxy_core::protocol::{MessagesAuth, Protocol};
    use llmproxy_store::ProviderPaths;
    use nonstream::{Database, MASTER_KEY, alias, fixtures, path};
    use support::{Gateway, Mock, respond};
    let config = configuration();
    let endpoint = config
        .get("OTEL_EXPORTER_OTLP_ENDPOINT")
        .expect("OTLP 出口已配置");
    let headers = config
        .get("OTEL_EXPORTER_OTLP_HEADERS")
        .expect("OTLP 认证已配置");
    let decoded: HashMap<_, _> = headers
        .split(',')
        .map(|header| {
            let (key, value) = header.split_once('=').expect("OTLP 头格式");
            (
                key.trim(),
                percent_encoding::percent_decode_str(value.trim())
                    .decode_utf8()
                    .expect("头编码")
                    .into_owned(),
            )
        })
        .collect();
    let authorization = config
        .get("LLMPROXY_OPENOBSERVE_QUERY_AUTHORIZATION")
        .expect(
            "查询认证需单独设置 LLMPROXY_OPENOBSERVE_QUERY_AUTHORIZATION，不能使用 OTLP 写入 token",
        );
    let stream = decoded.get("stream-name").expect("OpenObserve 日志流");
    assert!(
        stream
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    let service = format!("llmproxy-nonstream-acceptance-{}", std::process::id());
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    let (upstream, _) = Mock::http(|_, stream| {
        let mut body = fixtures::response(Protocol::AnthropicMessages, false);
        body["usage"] = serde_json::json!({
            "input_tokens":4,"output_tokens":3,"cache_read_input_tokens":4,"cache_creation_input_tokens":2,
            "cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":1}
        });
        respond(
            stream,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&body).unwrap(),
        );
    });
    let database = Database::new().await;
    database
        .add_provider(
            ProviderInput {
                name: "telemetry-mock".into(),
                paths: ProviderPaths::single(Protocol::AnthropicMessages),
                host: "127.0.0.1".into(),
                port: upstream.address.port(),
                tls: false,
                api_key: "telemetry-secret".into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::AnthropicMessages,
                anthropic_version: Some("2023-06-01".into()),
                messages_auth: MessagesAuth::ApiKey,
                connect_timeout_ms: 1500,
                read_timeout_ms: 1500,
                write_timeout_ms: 1500,
            },
            Protocol::AnthropicMessages,
            "upstream-model",
        )
        .await;
    let gateway = Gateway::database_with_environment(
        &database.url,
        MASTER_KEY,
        &[
            ("OTEL_SERVICE_NAME", &service),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
            ("OTEL_EXPORTER_OTLP_HEADERS", headers),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "3000"),
            ("OTEL_METRIC_EXPORT_INTERVAL", "1000"),
        ],
    );
    let alias = alias(Protocol::OpenAiChat, Protocol::AnthropicMessages);
    let mut request = fixtures::request(Protocol::OpenAiChat, &alias, "private-acceptance-prompt");
    request["metadata"] = serde_json::json!({"private_hint":"private-acceptance-metadata"});
    let response = gateway.request(
        "POST",
        &path(Protocol::OpenAiChat, &alias),
        "Content-Type: application/json\r\n",
        &serde_json::to_vec(&request).unwrap(),
    );
    assert_eq!(response.status, 200);
    response.body();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut stored = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let end = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        let response = client.post(format!("{}/_search?type=logs", endpoint.trim_end_matches('/')))
            .header("authorization", authorization).json(&serde_json::json!({"query":{
                "sql":format!("SELECT * FROM \"{stream}\""),"start_time":start,"end_time":end,"from":0,"size":500
            }})).send().await.expect("OpenObserve 查询连接失败");
        assert!(
            response.status().is_success(),
            "OpenObserve 查询 HTTP {}",
            response.status().as_u16()
        );
        let body: serde_json::Value = response.json().await.expect("OpenObserve 查询格式");
        stored = body["hits"]
            .as_array()
            .expect("OpenObserve 查询结果")
            .iter()
            .filter(|hit| {
                hit.as_object()
                    .is_some_and(|fields| fields.values().any(|v| v.as_str() == Some(&service)))
            })
            .cloned()
            .collect();
        if stored
            .iter()
            .any(|hit| observed_field(hit, "event_kind") == &serde_json::json!("usage"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let usage = stored
        .iter()
        .find(|hit| observed_field(hit, "event_kind") == &serde_json::json!("usage"))
        .expect("OpenObserve 未存储用量事件");
    for (key, expected) in [
        ("input_tokens", 10),
        ("output_tokens", 3),
        ("total_tokens", 13),
        ("cache_read_input_tokens", 4),
        ("cache_write_input_tokens", 2),
        ("cache_write_short_input_tokens", 1),
        ("cache_write_long_input_tokens", 1),
    ] {
        assert_eq!(
            observed_field(usage, key),
            &serde_json::json!(expected),
            "{key}"
        );
    }
    let trace = observed_field(usage, "trace_id")
        .as_str()
        .expect("用量事件的 trace ID");
    assert!(!trace.is_empty());
    assert!(stored.iter().any(|hit| observed_field(hit, "event_kind")
        == &serde_json::json!("request")
        && observed_field(hit, "trace_id").as_str() == Some(trace)));
    assert!(stored.iter().any(|hit| observed_field(hit, "event_kind")
        == &serde_json::json!("conversion")
        && observed_field(hit, "trace_id").as_str() == Some(trace)));
    let serialized = serde_json::to_string(&stored).unwrap();
    for private in [
        "private-acceptance-prompt",
        "private-acceptance-metadata",
        "telemetry-secret",
    ] {
        assert!(!serialized.contains(private));
    }
    // 保持进程存活，等待批量 trace exporter 和周期性 metric exporter 实际发送。
    for (kind, sql) in [
        (
            "traces",
            format!("SELECT * FROM \"{stream}\" WHERE trace_id = '{trace}'"),
        ),
        (
            "metrics",
            format!(
                "SELECT * FROM llmproxy_requests WHERE service_name = '{service}' AND protocol = 'openai_chat'"
            ),
        ),
    ] {
        loop {
            let end = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as u64;
            let response = client.post(format!("{}/_search?type={kind}", endpoint.trim_end_matches('/')))
                .header("authorization", authorization)
                .json(&serde_json::json!({"query":{"sql":sql,"start_time":start,"end_time":end,"from":0,"size":50}}))
                .send().await.expect("OpenObserve 遥测查询连接失败");
            assert!(
                response.status().is_success(),
                "OpenObserve {kind} 查询 HTTP {}",
                response.status().as_u16()
            );
            let body: serde_json::Value = response.json().await.expect("OpenObserve 遥测查询格式");
            let hits = body["hits"].as_array().expect("OpenObserve 遥测查询结果");
            if !hits.is_empty() {
                if kind == "metrics" {
                    assert!(hits.iter().any(|hit| hit["value"].as_f64() == Some(1.0)));
                }
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "OpenObserve 未存储 {kind}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    println!(
        "OpenObserve PASS logs/traces/metrics; usage: input=10 output=3 total=13 cache_read=4 cache_write=2 short=1 long=1; request/conversion/usage share trace; private content absent"
    );
}

/// OpenObserve 将 OTLP 属性扁平化；兼容直接字段和带属性前缀的字段名。
fn observed_field<'a>(hit: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    hit.get(name)
        .or_else(|| {
            hit.as_object()?
                .iter()
                .find(|(key, _)| key.ends_with(&format!("_{name}")))
                .map(|(_, value)| value)
        })
        .unwrap_or(&serde_json::Value::Null)
}

#[tokio::test]
#[ignore = "只读访问已配置的测试数据库，需要网络"]
async fn configured_provider_inventory() {
    let config = configuration();
    let mut db = toasty::Db::builder()
        .connect(database_url(&config))
        .await
        .unwrap_or_else(|_| panic!("数据库不可达"));
    let tables = toasty::sql::query("SELECT table_schema::text, table_name::text FROM information_schema.tables WHERE table_name IN ('providers', 'model_mappings', 'route_bindings', 'store_keys') ORDER BY table_schema, table_name")
        .exec(&mut db).await.expect("检查测试库结构");
    println!("test database tables: {tables:?}");
    let store = ProviderStore::connect(
        database_url(&config),
        config.get("LLMPROXY_MASTER_KEY").expect("主密钥已配置"),
    )
    .await
    .unwrap_or_else(|_| panic!("连接测试数据库失败（已隐藏连接信息）"));
    let models = store.list_models().await.expect("读取模型列表");
    for model in models.into_iter().filter(|model| model.provider_enabled) {
        println!(
            "model_id={} provider={} alias={} upstream={} protocols={:?}",
            model.id, model.provider_name, model.alias, model.upstream_model_id, model.protocols
        );
    }
}

/// 真实验收共用只读配置克隆，原数据库与 Provider 凭据不进入输出。
async fn live_database(config: &HashMap<String, String>) -> nonstream::Database {
    use nonstream::{ALL, Database};
    let master_key = config.get("LLMPROXY_MASTER_KEY").expect("主密钥已配置");
    let store = ProviderStore::connect(database_url(config), master_key)
        .await
        .unwrap_or_else(|_| panic!("连接配置库失败"));
    let mut models = store.list_models().await.expect("读取已配置模型");
    let providers = store.list().await.expect("读取 Provider 配置");
    // 优先较小文本模型；最终可用性由真实请求验证，不能把配置声明当成验收成功。
    models.sort_by_key(|model| {
        let name = model.upstream_model_id.to_ascii_lowercase();
        (
            !(["mini", "flash", "haiku"]
                .iter()
                .any(|word| name.contains(word))),
            model.id,
        )
    });
    let database = Database::with_master_key(master_key).await;
    for target in ALL {
        let model = models
            .iter()
            .find(|model| model.provider_enabled && model.protocols.contains(&target))
            .unwrap_or_else(|| panic!("缺少可用的 {} 模型配置", target.as_str()));
        let loaded = store
            .load_model_route(model.id, target)
            .await
            .expect("读取目标模型");
        let active = loaded.provider.expect("Provider 必须已启用");
        let provider = providers
            .iter()
            .find(|provider| provider.id == active.id)
            .expect("对应 Provider");
        database
            .add_provider(
                ProviderInput {
                    name: format!("live-{}", target.as_str()),
                    paths: provider.paths.clone(),
                    host: active.host,
                    port: active.port,
                    tls: active.tls,
                    api_key: active.secret,
                    enabled: true,
                    models_path: provider.models_path.clone(),
                    models_protocol: provider.models_protocol,
                    anthropic_version: active.anthropic_version,
                    messages_auth: active.messages_auth,
                    connect_timeout_ms: active.connect_timeout_ms,
                    read_timeout_ms: active.read_timeout_ms,
                    write_timeout_ms: active.write_timeout_ms,
                },
                target,
                &loaded.upstream_model_id,
            )
            .await;
        println!(
            "selected {}: {} / {}",
            target.as_str(),
            model.provider_name,
            model.upstream_model_id
        );
    }
    database
}

/// 四种客户端的真实 Schema／图片／PDF，以及有原生载体的音视频输入。
/// 使用当前已配置 Gemini 模型；校验实际内容和用量，不能仅凭 HTTP 200 判断成功。
#[tokio::test]
#[ignore = "真实多模态与结构化输出联调，显式运行会产生 token 费用"]
async fn configured_gemini_nonstream_capabilities() {
    use llmproxy_core::{ir::message::PartKind, protocol::Protocol};
    use nonstream::{
        ALL, alias,
        fixtures::{self, MediaCase},
        path,
    };
    let config = configuration();
    if let Some(pair) = config.get("LLMPROXY_LIVE_PAIR") {
        assert!(
            ALL.iter()
                .any(|source| pair == &format!("{}:gemini", source.as_str())),
            "多模态专项的目标必须是 Gemini，来源必须是已知协议"
        );
    }
    if let Some(capability) = config.get("LLMPROXY_LIVE_CAPABILITY") {
        assert!(
            ["structured", "image", "pdf", "audio", "video"].contains(&capability.as_str()),
            "未知的专项筛选条件"
        );
    }
    let database = live_database(&config).await;
    let gateway =
        support::Gateway::database(&database.url, config.get("LLMPROXY_MASTER_KEY").unwrap());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap();
    let mut failures = Vec::new();
    let mut attempted = 0;
    for source in ALL {
        if config
            .get("LLMPROXY_LIVE_PAIR")
            .is_some_and(|pair| pair != &format!("{}:gemini", source.as_str()))
        {
            continue;
        }
        let alias = alias(source, Protocol::Gemini);
        let cases = [
            None,
            Some(MediaCase::Image),
            Some(MediaCase::Pdf),
            Some(MediaCase::Audio),
            Some(MediaCase::Video),
        ];
        for case in cases
            .into_iter()
            .filter(|case| case.is_none_or(|case| case.supported(source)))
        {
            let label = match case {
                None => "structured",
                Some(MediaCase::Image) => "image",
                Some(MediaCase::Pdf) => "pdf",
                Some(MediaCase::Audio) => "audio",
                Some(MediaCase::Video) => "video",
            };
            if config
                .get("LLMPROXY_LIVE_CAPABILITY")
                .is_some_and(|selected| selected != label)
            {
                continue;
            }
            attempted += 1;
            let mut request = match case {
                None => fixtures::structured_request(source, &alias),
                Some(case) => fixtures::media_request(source, &alias, case),
            };
            // 媒体理解的动态思考也计入输出上限，给出充足但有界的测试预算。
            match source {
                Protocol::OpenAiChat => request["max_completion_tokens"] = serde_json::json!(512),
                Protocol::OpenAiResponses => request["max_output_tokens"] = serde_json::json!(512),
                Protocol::AnthropicMessages => request["max_tokens"] = serde_json::json!(512),
                Protocol::Gemini => {
                    request["generationConfig"]["maxOutputTokens"] = serde_json::json!(512)
                }
            }
            let result = async {
                let response = client
                    .post(format!(
                        "http://{}{}",
                        gateway.address,
                        path(source, &alias)
                    ))
                    .header("content-type", "application/json")
                    .body(serde_json::to_vec(&request).unwrap())
                    .send()
                    .await
                    .map_err(|_| "连接或读取超时".to_owned())?;
                let status = response.status();
                if !status.is_success() {
                    return Err(format!("HTTP {}", status.as_u16()));
                }
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|_| "读取响应失败".to_owned())?;
                let response = fixtures::decode_response(source, &bytes)?;
                let text: String = response
                    .messages
                    .iter()
                    .flat_map(|message| &message.parts)
                    .filter_map(|part| match &part.kind {
                        PartKind::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let correct = match case {
                    None => serde_json::from_str::<serde_json::Value>(&text)
                        .is_ok_and(|body| body == serde_json::json!({"answer":"OK"})),
                    Some(MediaCase::Image | MediaCase::Video) => {
                        text.to_ascii_lowercase().contains("blue")
                    }
                    Some(MediaCase::Pdf) => text.contains("HELLO"),
                    Some(MediaCase::Audio) => text.to_ascii_lowercase().contains("hello"),
                };
                if !correct {
                    return Err("输出未满足样本预期（已隐藏正文）".into());
                }
                let usage = response.usage.ok_or("没有报告用量")?;
                if usage.input_tokens.is_none_or(|count| count == 0)
                    || usage.output_tokens.is_none_or(|count| count == 0)
                {
                    return Err("缺少实际输入或输出用量".into());
                }
                println!(
                    "PASS {:?} {} -> gemini input={:?} output={:?}",
                    case,
                    source.as_str(),
                    usage.input_tokens,
                    usage.output_tokens
                );
                Ok::<_, String>(())
            }
            .await;
            if let Err(error) = result {
                failures.push(format!("{:?} {}: {error}", case, source.as_str()));
            }
        }
    }
    drop(gateway);
    assert!(attempted > 0, "筛选组合没有可验收的原生能力");
    assert!(
        failures.is_empty(),
        "真实专项未通过：{}",
        failures.join("; ")
    );
}

/// 每个目标协议选择一个已启用模型，共发出 16 次文本和 16 个工具双回合请求。
/// 原 PostgreSQL 只读，路由与凭据副本仅写入受限权限的临时 SQLite。
#[tokio::test]
#[ignore = "真实 Provider 联调，显式运行会产生 token 费用"]
async fn configured_provider_nonstream_matrix() {
    use llmproxy_core::{ir::message::PartKind, protocol::Protocol};
    use nonstream::{ALL, alias, fixtures, path};
    let config = configuration();
    let master_key = config.get("LLMPROXY_MASTER_KEY").expect("主密钥已配置");
    let pair = config.get("LLMPROXY_LIVE_PAIR");
    if let Some(pair) = pair {
        assert!(
            ALL.iter().any(|source| ALL
                .iter()
                .any(|target| pair == &format!("{}:{}", source.as_str(), target.as_str()))),
            "联调组合必须是已知协议"
        );
    }
    let database = live_database(&config).await;
    let gateway = support::Gateway::database(&database.url, master_key);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap();
    let mut failures = Vec::new();
    for target in ALL {
        for source in ALL {
            if pair.is_some_and(|pair| pair != &format!("{}:{}", source.as_str(), target.as_str()))
            {
                continue;
            }
            let alias = alias(source, target);
            for case in ["text", "tool"] {
                let result = async {
                    let request = if case == "tool" {
                        // 基础互通验收使用自动选择；具名强制选择单独记录 Provider 能力限制。
                        let mut body = fixtures::tool_request(source, &alias);
                        match source {
                            Protocol::OpenAiChat => {
                                body["tool_choice"] = serde_json::json!("auto");
                                body["reasoning_effort"] = serde_json::json!("none");
                            }
                            Protocol::OpenAiResponses => {
                                body["tool_choice"] = serde_json::json!("auto");
                                body["reasoning"] = serde_json::json!({"effort":"none"});
                            }
                            Protocol::AnthropicMessages => {
                                body["tool_choice"] = serde_json::json!({"type":"auto"});
                                body["thinking"] = serde_json::json!({"type":"disabled"});
                            }
                            Protocol::Gemini => {
                                body["toolConfig"] = serde_json::json!({"functionCallingConfig":{"mode":"AUTO"}});
                                body["generationConfig"]["thinkingConfig"] = serde_json::json!({"thinkingBudget":0});
                            }
                        }
                        if target == Protocol::Gemini {
                            // 所选 3.5 模型不接受 thinkingBudget=0；单回合工具入口使用默认思考模式。
                            match source {
                                Protocol::OpenAiChat => { body.as_object_mut().unwrap().remove("reasoning_effort"); }
                                Protocol::OpenAiResponses => { body.as_object_mut().unwrap().remove("reasoning"); }
                                Protocol::AnthropicMessages => { body.as_object_mut().unwrap().remove("thinking"); }
                                Protocol::Gemini => { body["generationConfig"].as_object_mut().unwrap().remove("thinkingConfig"); }
                            }
                        }
                        body
                } else {
                    fixtures::request(source, &alias, "Reply with exactly OK.")
                };
                let response = client
                    .post(format!(
                        "http://{}{}",
                        gateway.address,
                        path(source, &alias)
                    ))
                    .header("content-type", "application/json")
                    .body(
                        serde_json::to_vec(&request).unwrap(),
                    )
                    .send()
                    .await
                    .map_err(|_| "HTTP 连接或超时失败".to_owned())?;
                let status = response.status();
                if !status.is_success() {
                    return Err(format!("HTTP {}", status.as_u16()));
                }
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|_| "读取响应失败".to_owned())?;
                let ir = fixtures::decode_response(source, &bytes).map_err(str::to_owned)?;
                if case == "tool" {
                    let mut calls = ir.messages.iter().flat_map(|message| &message.parts).filter_map(|part| match &part.kind {
                        PartKind::ToolCall(call) => Some(call), _ => None,
                    }).chain(ir.items.iter().filter_map(|item| match item {
                        llmproxy_core::ir::response::Item::ToolCall {call,..} => Some(call), _ => None,
                    }));
                    let call = calls.next().ok_or("没有收到函数调用")?;
                    if call.name != "lookup" || calls.next().is_some() { return Err("函数调用名称或数量不一致".into()); }
                    let arguments = if let serde_json::Value::String(raw) = &call.arguments {
                        serde_json::from_str(raw).map_err(|_| "函数参数不是 JSON 对象")?
                    } else { call.arguments.clone() };
                    if arguments["q"] != "test" { return Err("函数参数未保留".into()); }
                    let next = fixtures::tool_result_request(source, &alias, &request, &ir, call)?;
                    let response = client.post(format!("http://{}{}", gateway.address, path(source, &alias)))
                        .header("content-type", "application/json")
                        .body(serde_json::to_vec(&next).unwrap()).send().await.map_err(|_| "工具结果回传连接失败")?;
                    if !response.status().is_success() { return Err(format!("工具结果回传 HTTP {}", response.status().as_u16())); }
                    let bytes = response.bytes().await.map_err(|_| "工具结果回传响应读取失败")?;
                    let completed = fixtures::decode_response(source, &bytes)?;
                    if !completed.messages.iter().flat_map(|message| &message.parts).any(|part| matches!(&part.kind, PartKind::Text(text) if text.contains("OK"))) {
                        return Err("工具结果未进入最终回答".into());
                    }
                    if completed.usage.as_ref().is_none_or(|usage| usage.input_tokens.is_none() || usage.output_tokens.is_none()) {
                        return Err("工具结果回合未报告输入输出用量".into());
                    }
                } else if !ir.messages.iter().flat_map(|message| &message.parts).any(
                    |part| matches!(&part.kind, PartKind::Text(text) if text.trim().contains("OK")),
                ) {
                    return Err("没有收到预期文本（可能受到模型思考预算或截断影响）".into());
                }
                if source != target
                    && source != Protocol::Gemini
                    && ir.model.as_deref() != Some(&alias)
                {
                    return Err("客户端模型别名不一致".into());
                }
                let usage = ir.usage.ok_or("Provider 未报告用量")?;
                if usage.input_tokens.is_none() || usage.output_tokens.is_none() {
                    return Err("用量缺少输入或输出计数".into());
                }
                println!(
                    "PASS {case} {} -> {} input={:?} output={:?} cache_read={:?}",
                    source.as_str(),
                    target.as_str(),
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache.read_input_tokens
                );
                Ok::<_, String>(())
            }
            .await;
                if let Err(reason) = result {
                    failures.push(format!(
                        "{case} {} -> {}: {reason}",
                        source.as_str(),
                        target.as_str()
                    ));
                }
            }
        }
    }
    // 先正常销毁网关，失败报告不会触发辅助设施打印真实请求的完整日志。
    drop(gateway);
    assert!(
        failures.is_empty(),
        "真实验收未通过：{}",
        failures.join("; ")
    );
}
