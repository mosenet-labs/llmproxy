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

/// 只报告固定的错误类别，不输出可能包含凭据、提示词或请求内容的 Provider 正文。
async fn provider_failure(response: reqwest::Response) -> String {
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    let reason = if text.contains("quota")
        && (text.contains("limit: 0") || text.contains("\"quotavalue\":\"0\""))
    {
        "模型配额为零"
    } else if text.contains("region") || text.contains("location is not supported") {
        "地区限制"
    } else if text.contains("insufficient credits") || text.contains("credit limit") {
        "余额或消费限额"
    } else if text.contains("resource_exhausted") || text.contains("rate limit") {
        "配额或限流"
    } else if text.contains("permission_denied") || text.contains("permission_error") {
        "权限拒绝"
    } else {
        "未分类的 Provider 拒绝"
    };
    format!("HTTP {status}（{reason}）")
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
        let selected_id = config
            .get(&format!(
                "LLMPROXY_LIVE_MODEL_ID_{}",
                target.as_str().to_ascii_uppercase()
            ))
            .map(|value| value.parse::<i64>().expect("测试模型 ID 必须是整数"));
        let model = models
            .iter()
            .find(|model| {
                model.provider_enabled
                    && model.protocols.contains(&target)
                    && selected_id.is_none_or(|id| model.id == id)
            })
            .unwrap_or_else(|| panic!("缺少可用的 {} 模型配置", target.as_str()));
        let loaded = store
            .load_model_route(model.id, target)
            .await
            .expect("读取目标模型");
        // 仅覆盖临时验收路由的模型名；必须同时指定配置 ID，避免误用其他 Provider。
        let upstream_model = config
            .get(&format!(
                "LLMPROXY_LIVE_UPSTREAM_MODEL_{}",
                target.as_str().to_ascii_uppercase()
            ))
            .map(|name| {
                assert!(selected_id.is_some(), "覆盖模型名时必须指定配置模型 ID");
                assert!(!name.trim().is_empty(), "验收模型名不能为空");
                name.as_str()
            })
            .unwrap_or(&loaded.upstream_model_id);
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
                upstream_model,
            )
            .await;
        println!(
            "selected {}: {} / {}",
            target.as_str(),
            model.provider_name,
            upstream_model
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

/// 验证当前 Gemini 模型实际执行代码或使用推理配置，HTTP 200 不作为唯一成功条件。
/// 参考：https://ai.google.dev/api/generate-content#Tool
/// 参考：https://ai.google.dev/api/generate-content#ThinkingConfig
#[tokio::test]
#[ignore = "真实服务端工具与推理联调，显式运行会产生 token 费用"]
async fn configured_gemini_native_capabilities() {
    use llmproxy_core::ir::{message::PartKind, response::Item};
    use llmproxy_core::protocol::Protocol;
    use nonstream::{ALL, alias, fixtures, path};
    let config = configuration();
    if let Some(pair) = config.get("LLMPROXY_LIVE_PAIR") {
        assert!(
            ALL.iter()
                .any(|source| pair == &format!("{}:gemini", source.as_str())),
            "专项的目标必须是 Gemini"
        );
    }
    let selected = config.get("LLMPROXY_LIVE_CAPABILITY").map(String::as_str);
    assert!(
        selected.is_none_or(|value| matches!(value, "code" | "reasoning" | "budget")),
        "未知专项筛选条件"
    );
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
        for capability in ["code", "reasoning", "budget"] {
            if selected.is_some_and(|selected| selected != capability)
                || (capability == "code" && source == Protocol::OpenAiChat)
                // 默认等级模型可能不接受数字预算；该专项需显式选择支持预算的模型。
                || (capability == "budget" && selected.is_none())
                || (capability == "budget"
                    && !matches!(source, Protocol::AnthropicMessages | Protocol::Gemini))
            {
                continue;
            }
            attempted += 1;
            let alias = alias(source, Protocol::Gemini);
            let body = match capability {
                "code" => fixtures::code_request(source, &alias),
                "budget" => fixtures::reasoning_budget_request(source, &alias),
                _ => fixtures::reasoning_request(source, &alias),
            };
            let result = async {
                let response = client.post(format!("http://{}{}", gateway.address, path(source, &alias)))
                    .header("content-type", "application/json").body(serde_json::to_vec(&body).unwrap()).send().await.map_err(|_| "连接或读取超时")?;
                if !response.status().is_success() {return Err(provider_failure(response).await);}
                let bytes = response.bytes().await.map_err(|_| "读取响应失败")?;
                let ir = fixtures::decode_response(source, &bytes)?;
                let text = ir.messages.iter().flat_map(|m| &m.parts).filter_map(|part| match &part.kind {
                    PartKind::Text(text) => Some(text.as_str()),
                    PartKind::ServerOutput(output) => output.text.as_deref(), _ => None,
                }).chain(ir.items.iter().filter_map(|item| match item {
                    Item::ServerOutput(output) => output.text.as_deref(), _ => None,
                })).collect::<String>();
                if !text.contains("1073") {return Err("未返回计算结果（已隐藏正文）".into());}
                if ir.messages.iter().flat_map(|m| &m.parts).any(|p| matches!(p.kind, PartKind::ToolCall(_))) || ir.items.iter().any(|item| matches!(item, Item::ToolCall {..})) {
                    return Err("服务端执行被误表达为客户端工具调用".into());
                }
                if capability == "code" {
                    let has_execution = if source == Protocol::Gemini {
                        ir.messages.iter().flat_map(|m| &m.parts).any(|p| matches!(&p.kind, PartKind::ServerOutput(output) if output.kind == llmproxy_core::ir::server_output::Kind::ExecutionResult))
                    } else {
                        // 目标客户端只收到可见文本；独立标记确保不是模型直接口算的最终正文。
                        text.contains("OUTCOME_OK") && text.contains("代码")
                    };
                    if !has_execution {return Err("未观察到真实代码执行记录".into());}
                }
                let usage = ir.usage.ok_or("缺少用量")?;
                if usage.input_tokens.is_none_or(|n| n == 0) || usage.output_tokens.is_none_or(|n| n == 0) {return Err("缺少实际输入输出用量".into());}
                if matches!(capability, "reasoning" | "budget") && usage.output_details.reasoning_tokens.is_none_or(|n| n == 0) {return Err("未报告实际推理用量".into());}
                println!("PASS {capability} {} -> gemini input={:?} output={:?} reasoning={:?}", source.as_str(), usage.input_tokens, usage.output_tokens, usage.output_details.reasoning_tokens);
                Ok::<_, String>(())
            }.await;
            if let Err(error) = result {
                failures.push(format!("{capability} {}: {error}", source.as_str()));
            }
        }
    }
    drop(gateway);
    assert!(attempted > 0, "筛选组合没有可验收能力");
    assert!(
        failures.is_empty(),
        "真实专项未通过：{}",
        failures.join("; ")
    );
}

/// 显式断点的短/长 TTL 分别创建缓存，再重复完全相同的前缀验证命中。
/// 不等待 TTL 到期；核对 Provider 返回的 TTL 写入桶，不将模拟计数当作真实命中。
/// 参考：https://platform.claude.com/docs/en/build-with-claude/prompt-caching
#[tokio::test]
#[ignore = "需要支持缓存写入的原厂 Messages 模型；显式运行产生约四次长输入费用"]
async fn configured_messages_cache_creation_and_hit() {
    use llmproxy_core::protocol::Protocol;
    use nonstream::{alias, fixtures, path};
    let mut config = configuration();
    let key = "LLMPROXY_LIVE_MODEL_ID_ANTHROPIC_MESSAGES";
    if !config.contains_key(key) {
        let store = ProviderStore::connect(
            database_url(&config),
            config.get("LLMPROXY_MASTER_KEY").expect("主密钥"),
        )
        .await
        .unwrap_or_else(|_| panic!("配置库连接失败"));
        let models = store.list_models().await.expect("读取已配置模型");
        let model = models.iter().find(|model| model.provider_enabled && model.protocols.contains(&Protocol::AnthropicMessages) && model.upstream_model_id.to_ascii_lowercase().contains("claude"))
            .expect("缺少支持真实缓存创建的 Messages 模型；请配置 Claude 模型或设置 LLMPROXY_LIVE_MODEL_ID_ANTHROPIC_MESSAGES");
        config.insert(key.into(), model.id.to_string());
    }
    let database = live_database(&config).await;
    let gateway =
        support::Gateway::database(&database.url, config.get("LLMPROXY_MASTER_KEY").unwrap());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap();
    let alias = alias(Protocol::AnthropicMessages, Protocol::AnthropicMessages);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut failures = Vec::new();
    for ttl in ["5m", "1h"] {
        // 超过当前 Claude 的最低缓存前缀长度；内容为无用户数据的合成记录。
        let prefix = format!(
            "Synthetic cache acceptance {nonce} {ttl}. Ignore the records and answer the final user question.\n"
        ) + &(0..1100)
            .map(|index| {
                format!("Record {index}: amber birch cedar delta ember forest granite harbor.\n")
            })
            .collect::<String>();
        let mut body = fixtures::request(
            Protocol::AnthropicMessages,
            &alias,
            "Reply with exactly OK.",
        );
        body["max_tokens"] = serde_json::json!(32);
        body["system"] = serde_json::json!([{"type":"text","text":prefix,"cache_control":{"type":"ephemeral","ttl":ttl}}]);
        for attempt in 0..2 {
            let result = async {
                let response = client.post(format!("http://{}{}", gateway.address, path(Protocol::AnthropicMessages, &alias)))
                    .header("content-type", "application/json").body(serde_json::to_vec(&body).unwrap()).send().await.map_err(|_| "请求超时")?;
                if !response.status().is_success() {return Err(provider_failure(response).await);}
                let bytes = response.bytes().await.map_err(|_| "读取响应失败")?;
                let ir = fixtures::decode_response(Protocol::AnthropicMessages, &bytes)?;
                if !ir.messages.iter().flat_map(|m| &m.parts).any(|part| matches!(&part.kind, llmproxy_core::ir::message::PartKind::Text(text) if text.trim() == "OK")) {return Err("回答不符合预期（已隐藏正文）".into());}
                let usage = ir.usage.ok_or("缺少用量")?;
                let bucket = if ttl == "5m" {usage.cache.write_short_input_tokens} else {usage.cache.write_long_input_tokens};
                if attempt == 0 && (usage.cache.write_input_tokens.is_none_or(|n| n == 0) || bucket.is_none_or(|n| n == 0)) {return Err("未发生真实缓存创建或未报告对应 TTL 写入桶".into());}
                if attempt == 1 && usage.cache.read_input_tokens.is_none_or(|n| n == 0) {return Err("重复前缀未产生真实缓存命中".into());}
                println!("PASS cache ttl={ttl} attempt={attempt} read={:?} write={:?} short={:?} long={:?}", usage.cache.read_input_tokens, usage.cache.write_input_tokens, usage.cache.write_short_input_tokens, usage.cache.write_long_input_tokens);
                Ok::<_, String>(())
            }.await;
            if let Err(error) = result {
                failures.push(format!("{ttl} attempt {attempt}: {error}"));
                break;
            }
        }
    }
    drop(gateway);
    assert!(
        failures.is_empty(),
        "缓存真实验收未通过：{}",
        failures.join("; ")
    );
}

/// 原生图片输出必须含可解码的真实图片字节，并保留模型报告的用量。
/// 图片生成模型必须显式选择；不使用普通文本模型的 HTTP 200 代替图片验收。
/// 参考：https://ai.google.dev/api/generate-content#GenerationConfig
/// 参考：https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite-image
#[tokio::test]
#[ignore = "需要 Gemini 图片模型；显式运行产生一次图片生成费用"]
async fn configured_gemini_image_output() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use llmproxy_core::{
        ir::{media::MediaSource, message::PartKind},
        protocol::Protocol,
    };
    use nonstream::{alias, fixtures, path};
    let config = configuration();
    assert!(
        config.contains_key("LLMPROXY_LIVE_MODEL_ID_GEMINI")
            || config.contains_key("LLMPROXY_LIVE_UPSTREAM_MODEL_GEMINI"),
        "图片专项必须显式选择模型"
    );
    let database = live_database(&config).await;
    let gateway =
        support::Gateway::database(&database.url, config.get("LLMPROXY_MASTER_KEY").unwrap());
    let alias = alias(Protocol::Gemini, Protocol::Gemini);
    let mut body = fixtures::request(
        Protocol::Gemini,
        &alias,
        "Generate one simple image of a solid blue square on a plain white background.",
    );
    body["generationConfig"] = serde_json::json!({"responseModalities":["TEXT","IMAGE"]});
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap()
        .post(format!(
            "http://{}{}",
            gateway.address,
            path(Protocol::Gemini, &alias)
        ))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&body).unwrap())
        .send()
        .await
        .unwrap_or_else(|_| panic!("图片请求连接或读取失败"));
    if !response.status().is_success() {
        let error = provider_failure(response).await;
        drop(gateway);
        panic!("图片请求 {error}");
    }
    let ir = fixtures::decode_response(Protocol::Gemini, &response.bytes().await.unwrap())
        .expect("图片响应解码");
    let image = ir
        .messages
        .iter()
        .flat_map(|m| &m.parts)
        .find_map(|part| match &part.kind {
            PartKind::Media(media) if media.kind == llmproxy_core::ir::media::MediaKind::Image => {
                Some(media)
            }
            _ => None,
        })
        .expect("未收到原生图片载体");
    let MediaSource::Base64 { data, mime_type } = &image.source else {
        panic!("图片未返回内联字节")
    };
    let bytes = STANDARD.decode(data).expect("图片 Base64 编码");
    assert!(
        match mime_type.as_deref() {
            Some("image/png") => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Some("image/jpeg") => bytes.starts_with(b"\xff\xd8\xff"),
            Some("image/webp") => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
            _ => false,
        },
        "图片 MIME 与文件头不符"
    );
    let usage = ir.usage.expect("缺少图片生成用量");
    assert!(usage.input_tokens.is_some_and(|n| n > 0));
    assert!(usage.output_tokens.is_some_and(|n| n > 0));
    println!(
        "PASS image output bytes={} input={:?} output={:?}",
        bytes.len(),
        usage.input_tokens,
        usage.output_tokens
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
