#[allow(dead_code)]
mod nonstream;
mod support;
use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::{
    ModelMappingInput, ModelRouteInput, ModelRouteTargetInput, ProviderInput, ProviderPaths,
    ProviderStore, VirtualKeyInput,
};
use nonstream::{Database, MASTER_KEY, fixtures};
use support::{DEADLINE, Gateway, Mock, respond, values};

async fn import_catalog(store: &ProviderStore, port: u16) -> llmproxy_store::ModelRouteView {
    let provider = if let Some(provider) = store
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|provider| provider.name == "same-provider")
    {
        provider
    } else {
        store
            .create(ProviderInput {
                name: "same-provider".into(),
                paths: ProviderPaths::single(Protocol::OpenAiChat),
                host: "127.0.0.1".into(),
                port,
                tls: false,
                api_key: "upstream-only".into(),
                enabled: true,
                models_path: "/models".into(),
                models_protocol: Protocol::OpenAiChat,
                anthropic_version: None,
                messages_auth: MessagesAuth::ApiKey,
                connect_timeout_ms: 1500,
                read_timeout_ms: 1500,
                write_timeout_ms: 1500,
            })
            .await
            .unwrap()
    };
    let model = store
        .create_model(ModelMappingInput {
            thinking: Default::default(),
            alias: "private-model".into(),
            provider_id: provider.id,
            upstream_model_id: format!("upstream-group-{}", store.group_id()),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
        })
        .await
        .unwrap();
    store
        .create_route(ModelRouteInput {
            name: "shared".into(),
            protocol: Protocol::OpenAiChat,
            provider_protocol: Protocol::OpenAiChat,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id: model.id,
                enabled: true,
            }],
        })
        .await
        .unwrap()
}

async fn configure(store: &ProviderStore, port: u16) -> llmproxy_store::ModelRouteView {
    let route = import_catalog(store, port).await;
    let version = store
        .list_groups()
        .await
        .unwrap()
        .into_iter()
        .find(|group| group.id == store.group_id())
        .unwrap()
        .version;
    store
        .set_group_resources(version, vec![route.targets[0].model.id], vec![route.id])
        .await
        .unwrap();
    route
}

#[tokio::test]
async fn keys_scope_catalog_calls_bridging_and_immediate_revocation() {
    let database = Database::new().await;
    let (upstream, requests) = Mock::http(|_, stream| {
        respond(
            stream,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&fixtures::response(Protocol::OpenAiChat, false)).unwrap(),
        )
    });
    let first = configure(&database.store, upstream.address.port()).await;
    let group = database.store.create_group("second").await.unwrap();
    let second = database.store.for_group(group.id);
    let other = configure(&second, upstream.address.port()).await;
    // 同名、未授权的 Responses 路由不能阻止或截获已授权 Chat 路由的桥接。
    let hidden = database
        .store
        .create_model(ModelMappingInput {
            thinking: Default::default(),
            alias: "hidden-responses".into(),
            provider_id: first.targets[0].model.provider_id,
            upstream_model_id: "unauthorized-response".into(),
            protocols: vec![Protocol::OpenAiChat],
            reference_price: None,
        })
        .await
        .unwrap();
    database
        .store
        .create_route(ModelRouteInput {
            name: "shared".into(),
            protocol: Protocol::OpenAiResponses,
            provider_protocol: Protocol::OpenAiChat,
            enabled: true,
            targets: vec![ModelRouteTargetInput {
                model_id: hidden.id,
                enabled: true,
            }],
        })
        .await
        .unwrap();
    let input = |id| VirtualKeyInput {
        name: "limited".into(),
        all_routes: false,
        model_ids: vec![],
        route_ids: vec![id],
        expires_at: None,
    };
    let key = database
        .store
        .create_virtual_key(input(first.id))
        .await
        .unwrap();
    let other_key = second.create_virtual_key(input(other.id)).await.unwrap();
    let empty = database
        .store
        .create_virtual_key(VirtualKeyInput {
            name: "empty".into(),
            all_routes: false,
            model_ids: vec![],
            route_ids: vec![],
            expires_at: None,
        })
        .await
        .unwrap();
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let auth =
        |key: &str| format!("Authorization: Bearer {key}\r\nContent-Type: application/json\r\n");
    for path in ["/models", "/v1/models", "/v1/chat/completions"] {
        assert_eq!(
            gateway
                .request_raw(
                    if path.ends_with("completions") {
                        "POST"
                    } else {
                        "GET"
                    },
                    path,
                    "",
                    b"{}"
                )
                .status,
            401
        );
    }
    for header in [
        format!("Authorization: Bearer {}\r\n", key.secret),
        format!("authorization: bearer {}\r\n", key.secret),
        format!("x-api-key: {}\r\n", key.secret),
        format!("x-goog-api-key: {}\r\n", key.secret),
        format!(
            "{}x-api-key: {}\r\nx-goog-api-key: {}\r\n",
            auth(&key.secret),
            key.secret,
            key.secret
        ),
    ] {
        assert_eq!(
            gateway.request_raw("GET", "/models", &header, b"").status,
            200
        );
    }
    for header in [
        "x-llmproxy-api-key",
        "x-virtual-key",
        "x-llmproxy-history-auth",
    ] {
        assert_eq!(
            gateway
                .request_raw(
                    "GET",
                    "/models",
                    &format!("{header}: {}\r\n", key.secret),
                    b""
                )
                .status,
            401
        );
    }
    assert_eq!(
        gateway
            .request_raw("GET", &format!("/models?key={}", key.secret), "", b"")
            .status,
        401
    );
    assert_eq!(
        gateway
            .request_raw(
                "GET",
                "/models",
                &format!("Authorization: Basic {}\r\n", key.secret),
                b""
            )
            .status,
        401
    );
    let response = gateway.request_raw("GET", "/models", &auth(&key.secret), b"");
    assert_eq!(response.status, 200);
    let catalog: serde_json::Value = serde_json::from_slice(&response.body()).unwrap();
    assert_eq!(catalog["data"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["data"][0]["id"], "shared");
    assert_eq!(
        catalog["data"][0]["protocols"].as_object().unwrap().len(),
        4
    );
    assert_eq!(
        gateway
            .request_raw("GET", "/models", &auth(&empty.secret), b"")
            .status,
        200
    );
    let body = br#"{"model":"shared","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":16}"#;
    for (key, expected) in [
        (&key.secret, "upstream-group-1"),
        (&other_key.secret, "upstream-group-2"),
    ] {
        assert_eq!(
            gateway
                .request_raw("POST", "/v1/chat/completions", &auth(key), body)
                .status,
            200
        );
        let request = requests.recv_timeout(DEADLINE).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap()["model"],
            expected
        );
        assert_eq!(
            values(&request.headers, "authorization"),
            ["Bearer upstream-only"]
        );
        assert!(values(&request.headers, "x-llmproxy-group").is_empty());
    }
    let forbidden = br#"{"model":"private-model","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":16}"#;
    assert_eq!(
        gateway
            .request_raw(
                "POST",
                "/v1/chat/completions",
                &auth(&key.secret),
                forbidden
            )
            .status,
        403
    );
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &auth(&empty.secret), body)
            .status,
        403
    );
    assert_eq!(
        gateway
            .request_raw(
                "POST",
                "/v1/responses",
                &auth(&key.secret),
                br#"{"model":"shared","input":"hi","max_output_tokens":16}"#
            )
            .status,
        200
    );
    let bridged = requests.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bridged.body).unwrap()["model"],
        "upstream-group-1"
    );
    let ambiguous = format!("{}x-api-key: {}\r\n", auth(&key.secret), other_key.secret);
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &ambiguous, body)
            .status,
        401
    );
    database
        .store
        .revoke_virtual_key(key.view.id, key.view.version)
        .await
        .unwrap();
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &auth(&key.secret), body)
            .status,
        401
    );
    let group = database
        .store
        .list_groups()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.id == group.id)
        .unwrap();
    second
        .update_group(group.id, group.version, "second", false)
        .await
        .unwrap();
    assert_eq!(
        gateway
            .request_raw("GET", "/models", &auth(&other_key.secret), b"")
            .status,
        401
    );
}

#[tokio::test]
async fn limited_keys_authorize_models_and_routes_independently() {
    let database = Database::new().await;
    let (upstream, requests) = Mock::http(|_, stream| {
        respond(
            stream,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&fixtures::response(Protocol::OpenAiChat, false)).unwrap(),
        )
    });
    let route = configure(&database.store, upstream.address.port()).await;
    let model = route.targets[0].model.id;
    // Identical numeric IDs across the two tables must not share grants.
    assert_eq!(model, route.id);
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let base = format!("http://{}", gateway.address);
    let client = reqwest::Client::new();
    let html = client
        .get(format!("{base}/ui/groups/1/keys"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("可调用资源"));
    assert!(html.contains("Models "));
    assert!(html.contains("Model Routes "));
    assert!(html.contains("name=\"model_id\""));
    let csrf = html
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let model_id = model.to_string();
    let route_id = route.id.to_string();
    let mut keys = Vec::new();
    for (name, models, routes) in [
        ("model-only", true, false),
        ("route-only", false, true),
        ("mixed", true, true),
    ] {
        let mut fields = vec![
            ("csrf", csrf),
            ("group_id", "1"),
            ("name", name),
            ("all_routes", "false"),
        ];
        if models {
            fields.push(("model_id", &model_id));
        }
        if routes {
            fields.push(("route_id", &route_id));
        }
        let result: serde_json::Value = procedure(&client, &base, "create-key", None, &fields)
            .await
            .json()
            .await
            .unwrap();
        let secret = result["ok"].as_str().unwrap().to_owned();
        let identity = database
            .store
            .authenticate_virtual_key(&secret)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            identity.model_ids,
            if models { vec![model] } else { vec![] }
        );
        assert_eq!(
            identity.route_ids,
            if routes { vec![route.id] } else { vec![] }
        );
        keys.push(secret);
    }
    let auth =
        |key: &str| format!("Authorization: Bearer {key}\r\nContent-Type: application/json\r\n");
    for (index, allowed) in [
        vec!["private-model"],
        vec!["shared"],
        vec!["private-model", "shared"],
    ]
    .into_iter()
    .enumerate()
    {
        let response = gateway.request_raw("GET", "/models", &auth(&keys[index]), b"");
        assert_eq!(response.status, 200);
        let catalog: serde_json::Value = serde_json::from_slice(&response.body()).unwrap();
        assert_eq!(
            catalog["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            allowed
        );
        for alias in ["private-model", "shared"] {
            let body = serde_json::to_vec(&serde_json::json!({"model": alias, "messages": [{"role": "user", "content": "hi"}], "max_completion_tokens": 16})).unwrap();
            assert_eq!(
                gateway
                    .request_raw("POST", "/v1/chat/completions", &auth(&keys[index]), &body)
                    .status,
                if allowed.contains(&alias) { 200 } else { 403 }
            );
            if allowed.contains(&alias) {
                requests.recv_timeout(DEADLINE).unwrap();
            }
            let body = serde_json::to_vec(
                &serde_json::json!({"model": alias, "input": "hi", "max_output_tokens": 16}),
            )
            .unwrap();
            assert_eq!(
                gateway
                    .request_raw("POST", "/v1/responses", &auth(&keys[index]), &body)
                    .status,
                if allowed.contains(&alias) { 200 } else { 403 }
            );
            if allowed.contains(&alias) {
                requests.recv_timeout(DEADLINE).unwrap();
            }
        }
    }
    let other = database.store.create_group("other").await.unwrap();
    let result: serde_json::Value = procedure(
        &client,
        &base,
        "create-key",
        Some(&format!("llmproxy_keys_group={}", other.id)),
        &[
            ("csrf", csrf),
            ("group_id", &other.id.to_string()),
            ("name", "cross-group"),
            ("model_id", &model_id),
        ],
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(result.get("err").is_some());
    assert!(
        database
            .store
            .for_group(other.id)
            .list_virtual_keys()
            .await
            .unwrap()
            .is_empty()
    );
    let version = database
        .store
        .list_groups()
        .await
        .unwrap()
        .into_iter()
        .find(|g| g.id == 1)
        .unwrap()
        .version;
    database
        .store
        .set_group_resources(version, vec![], vec![route.id])
        .await
        .unwrap();
    let body = br#"{"model":"private-model","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":16}"#;
    // Revocation must apply even before the routing snapshot refreshes.
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &auth(&keys[0]), body)
            .status,
        403
    );
    database
        .store
        .set_group_resources(version + 1, vec![model], vec![route.id])
        .await
        .unwrap();
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &auth(&keys[0]), body)
            .status,
        403
    );
    let mixed = database
        .store
        .authenticate_virtual_key(&keys[2])
        .await
        .unwrap()
        .unwrap();
    assert!(mixed.model_ids.is_empty());
    assert_eq!(mixed.route_ids, vec![route.id]);
}

#[tokio::test]
async fn console_separates_groups_and_keys_and_displays_secret_only_on_creation() {
    let database = Database::new().await;
    let group = database.store.create_group("second").await.unwrap();
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let base = format!("http://{}", gateway.address);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let html = client
        .get(format!("{base}/ui/groups"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("资源组列表"));
    assert!(html.contains("id=\"group-edit-1\""));
    assert!(html.contains(&format!("id=\"group-edit-{}\"", group.id)));
    assert!(!html.contains("id=\"key-create\""));
    assert!(html.contains("/ui/_topcoat/runtime/procedures/save-group"));
    assert!(!html.contains("action=\"/ui/groups/create\""));
    assert!(!html.contains("action=\"/ui/groups/update\""));
    let csrf = html
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    for page in [
        "/ui/providers",
        "/ui/models",
        "/ui/routes",
        "/ui/chat",
        "/ui/holidays",
        "/ui/subscriptions",
        "/ui/groups/1/keys",
        "/ui/groups/1/models",
        "/ui/groups",
    ] {
        let response = client.get(format!("{base}{page}")).send().await.unwrap();
        assert_eq!(response.status(), 200, "{page}");
        let html = response.text().await.unwrap();
        let header = html
            .split("id=\"console-header\"")
            .nth(1)
            .unwrap()
            .split("</header>")
            .next()
            .unwrap();
        assert!(!header.contains("name=\"group_id\""), "{page}");
        let scope = page.strip_prefix("/ui/").unwrap();
        if scope == "chat" {
            assert!(
                html.contains(&format!("data-group-selector=\"{scope}\"")),
                "{page}"
            );
            assert!(
                html.contains(&format!("name=\"return_to\" value=\"{page}\"")),
                "{page}"
            );
            let response = client
                .post(format!("{base}/ui/groups/select"))
                .form(&[
                    ("csrf", csrf),
                    ("group_id", &group.id.to_string()),
                    ("return_to", page),
                ])
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 303);
            assert_eq!(response.headers()["location"], page);
            assert!(
                response.headers()["set-cookie"]
                    .to_str()
                    .unwrap()
                    .starts_with(&format!("llmproxy_{scope}_group="))
            );
        } else {
            assert!(!html.contains("data-group-selector="), "{page}");
            assert!(!html.contains("切换到此组"), "{page}");
        }
    }
    for destination in [
        "https://example.invalid",
        "//example.invalid",
        "/ui/keys/create",
        "/ui/providers",
        "/ui/models",
        "/ui/routes",
        "/ui/groups",
    ] {
        let response = client
            .post(format!("{base}/ui/groups/select"))
            .form(&[
                ("csrf", csrf),
                ("group_id", &group.id.to_string()),
                ("return_to", destination),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 303);
        assert_eq!(response.headers()["location"], "/ui/groups");
        assert!(!response.headers().contains_key("set-cookie"));
    }
    let cookie = "llmproxy_keys_group=1; llmproxy_models_group=1; llmproxy_chat_group=1".to_owned();
    for suffix in ["models", "keys"] {
        let page = client
            .get(format!("{base}/ui/groups/{}/{suffix}", group.id))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(page.contains("所属资源组："));
        assert!(page.contains(&format!("data-current-group=\"{}\"", group.id)));
        assert!(page.contains("second"));
        assert!(!page.contains("data-group-selector="));
        assert!(!page.contains("id=\"nav-keys\""));
        let header = page
            .split("id=\"console-header\"")
            .nth(1)
            .unwrap()
            .split("</header>")
            .next()
            .unwrap();
        assert!(header.contains("href=\"/ui/groups\""));
        assert!(header.contains(">second详情</strong>"));
        assert!(page.contains("id=\"group-tab-models\""));
        assert!(page.contains("id=\"group-tab-keys\""));
    }
    for suffix in ["models", "keys"] {
        assert_eq!(
            client
                .get(format!("{base}/ui/groups/999/{suffix}"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    let legacy = client
        .get(format!("{base}/ui/keys"))
        .header("cookie", format!("llmproxy_keys_group={}", group.id))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 303);
    assert_eq!(
        legacy.headers()["location"],
        format!("/ui/groups/{}?tab=keys", group.id)
    );
    let response = procedure(
        &client,
        &base,
        "create-key",
        Some(&cookie),
        &[
            ("csrf", csrf),
            ("group_id", &group.id.to_string()),
            ("name", "Console client"),
            ("all_routes", "true"),
        ],
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(!response.headers().contains_key("location"));
    let result: serde_json::Value = response.json().await.unwrap();
    let secret = result["ok"].as_str().unwrap();
    assert!(secret.starts_with("lp-vk-"));
    assert_eq!(
        database
            .store
            .authenticate_virtual_key(secret)
            .await
            .unwrap()
            .unwrap()
            .group_id,
        group.id
    );
    let html = client
        .get(format!("{base}/ui/groups/{}/keys", group.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Console client"));
    assert!(!html.contains(secret));
    let unscoped = client
        .get(format!("{base}/ui/groups/1/keys"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!unscoped.contains("Console client"));
    let groups = client
        .get(format!("{base}/ui/groups"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(groups.contains("资源组列表"));
    assert!(!groups.contains("Console client"));
    assert!(!groups.contains(secret));
    assert!(html.contains("/ui/_topcoat/runtime/shards/key-workspace"));
    assert!(html.contains("/ui/_topcoat/runtime/procedures/create-key"));
    assert!(html.contains("/ui/_topcoat/runtime/procedures/change-key"));
    assert!(html.contains("Authorization: Bearer &lt;Key&gt;"));
    assert!(html.contains("x-api-key: &lt;Key&gt;"));
    assert!(html.contains("x-goog-api-key: &lt;Key&gt;"));
    assert!(!html.contains("action=\"/ui/keys/create\""));
    assert!(!html.contains("action=\"/ui/keys/change\""));
    assert!(!html.contains("window.location.replace('/ui/keys')"));
    let invalid = procedure(
        &client,
        &base,
        "create-key",
        Some(&cookie),
        &[
            ("csrf", csrf),
            ("group_id", &group.id.to_string()),
            ("name", "Draft retained"),
            ("expires_at", "invalid-date"),
        ],
    )
    .await;
    assert_eq!(invalid.status(), 200);
    assert_eq!(
        invalid.json::<serde_json::Value>().await.unwrap()["err"],
        "请选择有效的过期时间"
    );
    assert_eq!(
        database
            .store
            .for_group(group.id)
            .list_virtual_keys()
            .await
            .unwrap()
            .len(),
        1
    );
    let invalid = procedure(
        &client,
        &base,
        "save-group",
        Some(&cookie),
        &[
            ("csrf", csrf),
            ("group_id", &group.id.to_string()),
            ("version", &group.version.to_string()),
            ("name", "default"),
            ("enabled", "true"),
        ],
    )
    .await;
    assert_eq!(invalid.status(), 200);
    assert_eq!(
        invalid.json::<serde_json::Value>().await.unwrap()["err"],
        "已存在同名资源组，请使用其他名称"
    );
    let duplicate = procedure(
        &client,
        &base,
        "save-group",
        None,
        &[("csrf", csrf), ("name", " second ")],
    )
    .await;
    assert_eq!(duplicate.status(), 200);
    assert!(!duplicate.headers().contains_key("location"));
    assert_eq!(
        duplicate.json::<serde_json::Value>().await.unwrap()["err"],
        "已存在同名资源组，请使用其他名称"
    );
    assert_eq!(database.store.list_groups().await.unwrap().len(), 2);
    let response = procedure(
        &client,
        &base,
        "save-group",
        None,
        &[("csrf", csrf), ("name", "Created in place")],
    )
    .await;
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("location"));
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "组已创建"
    );
    let response = client
        .post(format!("{base}/ui/groups"))
        .header("x-topcoat-runtime", "true")
        .header("content-type", "application/json")
        .body(r#"{"signals":{}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let refreshed = response.text().await.unwrap();
    assert!(refreshed.contains("Created in place"));
    assert!(!refreshed.contains("data-group-selector="));
    let key = database
        .store
        .for_group(group.id)
        .list_virtual_keys()
        .await
        .unwrap()
        .remove(0);
    for action in ["disable", "enable"] {
        let response = procedure(
            &client,
            &base,
            "change-key",
            Some(&cookie),
            &[
                ("csrf", csrf),
                ("group_id", &group.id.to_string()),
                ("id", &key.id.to_string()),
                ("version", &key.version.to_string()),
                ("action", action),
            ],
        )
        .await;
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("location"));
        let outcome: serde_json::Value = response.json().await.unwrap();
        if action == "disable" {
            assert_eq!(outcome["ok"], "Key 已停用");
        } else {
            assert!(outcome["err"].is_string());
        }
    }
    assert!(
        !database
            .store
            .for_group(group.id)
            .list_virtual_keys()
            .await
            .unwrap()[0]
            .enabled
    );
    let signal = |id: u8, value: serde_json::Value| serde_json::json!({"t":"Signal","id":format!("{id:032x}"),"v":value});
    let response = client.post(format!("{base}/ui/_topcoat/runtime/shards/key-workspace"))
        .header("cookie", &cookie).header("content-type", "application/json")
        .body(serde_json::json!({"args":[{"t":"i64","bits":64,"v":group.id.to_string()},1.0,signal(1,serde_json::json!(1.0)),signal(2,serde_json::json!("")),signal(3,serde_json::json!(""))],"signals":{}}).to_string())
        .send().await.unwrap();
    assert_eq!(response.status(), 200);
    let refreshed = response.text().await.unwrap();
    assert!(refreshed.contains("Console client"));
    assert!(refreshed.contains("已停用"));
    assert!(!refreshed.contains(secret));
    let forbidden = procedure(
        &client,
        &base,
        "change-key",
        None,
        &[
            ("csrf", csrf),
            ("group_id", "1"),
            ("id", &key.id.to_string()),
            ("version", "1"),
            ("action", "disable"),
        ],
    )
    .await;
    assert!(forbidden.json::<serde_json::Value>().await.unwrap()["err"].is_string());
    let forbidden = procedure(
        &client,
        &base,
        "save-group",
        None,
        &[("csrf", "wrong"), ("name", "denied")],
    )
    .await;
    assert_eq!(forbidden.status(), 403);
    for path in [
        "/ui/groups/create",
        "/ui/groups/update",
        "/ui/keys/create",
        "/ui/keys/change",
    ] {
        let response = client
            .post(format!("{base}{path}"))
            .form(&[("csrf", csrf)])
            .send()
            .await
            .unwrap();
        assert!(matches!(response.status().as_u16(), 404 | 405), "{path}");
    }
}

#[tokio::test]
async fn group_resources_procedure_and_gateway_respect_independent_memberships() {
    let database = Database::new().await;
    let (upstream, requests) = Mock::http(|_, stream| {
        respond(
            stream,
            200,
            "Content-Type: application/json\r\n",
            &serde_json::to_vec(&fixtures::response(Protocol::OpenAiChat, false)).unwrap(),
        )
    });
    let route = import_catalog(&database.store, upstream.address.port()).await;
    let group = database.store.create_group("applications").await.unwrap();
    let scoped = database.store.for_group(group.id);
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let base = format!("http://{}", gateway.address);
    let client = reqwest::Client::new();
    let html = client
        .get(format!("{base}/ui/groups"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.find("id=\"nav-chat\"").unwrap() < html.find("id=\"nav-providers\"").unwrap());
    assert!(!html.contains("管理资源"));
    assert!(html.contains(&format!("href=\"/ui/groups/{}\"", group.id)));
    assert!(!html.contains(&format!("href=\"/ui/groups/{}/models\"", group.id)));
    assert!(!html.contains(&format!("href=\"/ui/groups/{}/keys\"", group.id)));
    assert!(!html.contains("/ui/_topcoat/runtime/procedures/save-group-resources"));
    let page = client
        .get(format!("{base}/ui/groups/{}/models", group.id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("添加模型或路由"));
    assert!(page.contains("/ui/_topcoat/runtime/procedures/save-group-resources"));
    assert!(page.contains("当前组还没有模型或路由"));
    let csrf = html
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(route.group_ids.is_empty());
    assert!(route.targets[0].model.group_ids.is_empty());
    assert!(database.store.list_models().await.unwrap().is_empty());
    assert!(database.store.list_routes().await.unwrap().is_empty());
    for path in ["models", "routes"] {
        let page = client
            .get(format!("{base}/ui/{path}"))
            .header("cookie", format!("llmproxy_{path}_group={}", group.id))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!page.contains("data-group-selector="));
        assert!(page.contains(if path == "models" {
            "private-model"
        } else {
            "shared"
        }));
    }
    let key = scoped
        .create_virtual_key(VirtualKeyInput {
            name: "app".into(),
            all_routes: true,
            model_ids: vec![],
            route_ids: vec![],
            expires_at: None,
        })
        .await
        .unwrap();
    let auth = format!(
        "Authorization: Bearer {}\r\nContent-Type: application/json\r\n",
        key.secret
    );
    let empty = gateway.request_raw("GET", "/models", &auth, b"");
    let empty: serde_json::Value = serde_json::from_slice(&empty.body()).unwrap();
    assert!(empty["data"].as_array().unwrap().is_empty());
    assert_eq!(
        gateway
            .request_raw(
                "POST",
                "/v1/chat/completions",
                &auth,
                br#"{"model":"shared","messages":[{"role":"user","content":"hi"}]}"#
            )
            .status,
        404
    );
    assert_eq!(upstream.count(), 0);
    let fields = [
        ("csrf", csrf),
        ("group_id", &group.id.to_string()),
        ("version", &group.version.to_string()),
        ("route_id", &route.id.to_string()),
    ];
    let response = procedure(&client, &base, "save-group-resources", None, &fields).await;
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("location"));
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["ok"],
        "组资源已更新"
    );
    assert!(scoped.list_models().await.unwrap().is_empty());
    assert_eq!(scoped.list_routes().await.unwrap()[0].id, route.id);
    assert_eq!(key.view.group_id, group.id);
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let response = gateway.request_raw("GET", "/models", &auth, b"");
        let catalog: serde_json::Value = serde_json::from_slice(&response.body()).unwrap();
        if catalog["data"]
            .as_array()
            .is_some_and(|items| items.len() == 1 && items[0]["id"] == "shared")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "membership snapshot did not refresh: {catalog}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let body = br#"{"model":"shared","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":16}"#;
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &auth, body)
            .status,
        200
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests.recv_timeout(DEADLINE).unwrap().body)
            .unwrap()["model"],
        "upstream-group-1"
    );
    let direct = br#"{"model":"private-model","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":16}"#;
    assert_eq!(
        gateway
            .request_raw("POST", "/v1/chat/completions", &auth, direct)
            .status,
        404
    );
    let denied = procedure(
        &client,
        &base,
        "save-group-resources",
        None,
        &[
            ("csrf", "invalid"),
            ("group_id", &group.id.to_string()),
            ("version", "0"),
        ],
    )
    .await;
    assert_eq!(denied.status(), 403);

    let group_id = group.id.to_string();
    let model_id = route.targets[0].model.id.to_string();
    let route_id = route.id.to_string();
    let version = (group.version + 1).to_string();
    let result: serde_json::Value = procedure(
        &client,
        &base,
        "save-group-resources",
        None,
        &[
            ("csrf", csrf),
            ("group_id", &group_id),
            ("version", &version),
            ("model_id", &model_id),
            ("route_id", &route_id),
        ],
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(result["ok"], "组资源已更新");
    let limited = scoped
        .create_virtual_key(VirtualKeyInput {
            name: "limited".into(),
            all_routes: false,
            model_ids: vec![route.targets[0].model.id],
            route_ids: vec![route.id],
            expires_at: None,
        })
        .await
        .unwrap();
    let page = client
        .post(format!("{base}/ui/groups/{}/models", group.id))
        .header("x-topcoat-runtime", "true")
        .header("content-type", "application/json")
        .body(r#"{"signals":{}}"#)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains(&format!("data-group-model=\"{model_id}\"")));
    assert!(page.contains(&format!("data-group-route=\"{route_id}\"")));
    assert!(page.contains("移出组"));
    assert!(page.contains("已加入"));
    let mut removal = vec![
        ("csrf", csrf),
        ("group_id", group_id.as_str()),
        ("version", version.as_str()),
        ("id", model_id.as_str()),
        ("kind", "model"),
    ];
    let stale: serde_json::Value =
        procedure(&client, &base, "remove-group-resource", None, &removal)
            .await
            .json()
            .await
            .unwrap();
    assert!(stale["err"].is_string());
    assert_eq!(scoped.list_models().await.unwrap().len(), 1);
    let fresh_version = (group.version + 2).to_string();
    removal[2].1 = &fresh_version;
    let removed = procedure(&client, &base, "remove-group-resource", None, &removal).await;
    assert!(!removed.headers().contains_key("location"));
    assert_eq!(
        removed.json::<serde_json::Value>().await.unwrap()["ok"],
        "资源已移出组"
    );
    assert!(scoped.list_models().await.unwrap().is_empty());
    assert_eq!(scoped.list_routes().await.unwrap().len(), 1);
    let identity = scoped
        .authenticate_virtual_key(&limited.secret)
        .await
        .unwrap()
        .unwrap();
    assert!(identity.model_ids.is_empty());
    assert_eq!(identity.route_ids, vec![route.id]);
    assert_eq!(scoped.list_all_models().await.unwrap().len(), 1);
    let denied = procedure(
        &client,
        &base,
        "remove-group-resource",
        None,
        &[
            ("csrf", "invalid"),
            ("group_id", &group_id),
            ("version", &version),
            ("id", &route_id),
            ("kind", "route"),
        ],
    )
    .await;
    assert_eq!(denied.status(), 403);
}

#[tokio::test]
async fn group_tables_paginate_with_runtime_signals_and_preserve_group_paths() {
    let database = Database::new().await;
    let mut groups = Vec::new();
    for index in 0..11 {
        groups.push(
            database
                .store
                .create_group(&format!("page-group-{index:02}"))
                .await
                .unwrap(),
        );
    }
    let scoped = database.store.for_group(groups[0].id);
    let route = import_catalog(&database.store, 1).await;
    let mut models = vec![route.targets[0].model.id];
    for index in 0..10 {
        models.push(
            database
                .store
                .create_model(ModelMappingInput {
                    alias: format!("page-model-{index:02}"),
                    provider_id: route.targets[0].model.provider_id,
                    upstream_model_id: format!("upstream-{index}"),
                    protocols: vec![Protocol::OpenAiChat],
                    thinking: Default::default(),
                    reference_price: None,
                })
                .await
                .unwrap()
                .id,
        );
    }
    let last_model = *models.last().unwrap();
    scoped
        .set_group_resources(groups[0].version, models, vec![route.id])
        .await
        .unwrap();
    for index in 0..11 {
        scoped
            .create_virtual_key(VirtualKeyInput {
                name: format!("page-key-{index:02}"),
                expires_at: None,
                all_routes: true,
                model_ids: vec![],
                route_ids: vec![],
            })
            .await
            .unwrap();
    }
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let client = reqwest::Client::new();
    let base = format!("http://{}", gateway.address);
    for (path, total, panel) in [
        ("/ui/groups".to_owned(), 12, ""),
        (format!("/ui/groups/{}", groups[0].id), 12, "models"),
        (format!("/ui/groups/{}?tab=keys", groups[0].id), 11, "keys"),
        (format!("/ui/groups/{}/models", groups[0].id), 12, "models"),
        (format!("/ui/groups/{}/keys", groups[0].id), 11, "keys"),
    ] {
        let url = format!("{base}{path}");
        let html = client.get(&url).send().await.unwrap().text().await.unwrap();
        assert!(html.contains("gr-data-table-compact"), "{path}");
        assert!(
            html.contains(&format!("显示 1–10 条，共 {total} 条")),
            "{path}"
        );
        assert!(html.contains("每页记录数"));
        let table_html = group_panel(&html, panel);
        let body = table_html
            .split("<tbody>")
            .nth(1)
            .unwrap()
            .split("</tbody>")
            .next()
            .unwrap();
        assert_eq!(body.matches("<tr").count(), 10, "{path}");
        let mut signals = pagination_signals(&html);
        let page_id = click_signal(table_html, "aria-label=\"第 1 页\"");
        let size_id = bound_signal(table_html, "每页记录数");
        signals[&page_id]["v"] = "2".into();
        let second = client
            .post(&url)
            .header("x-topcoat-runtime", "true")
            .json(&serde_json::json!({"signals":signals}))
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 200, "{path}");
        let second = second.text().await.unwrap();
        assert!(
            group_panel(&second, panel).contains(&format!("显示 11–{total} 条，共 {total} 条")),
            "{path}"
        );
        if panel == "models" {
            assert!(!body.contains(&format!("data-group-model=\"{last_model}\"")));
            assert!(second.contains(&format!("data-group-model=\"{last_model}\"")));
            assert!(second.contains(&format!("data-group-route=\"{}\"", route.id)));
            let query_id = bound_signal(&html, "搜索组内资源");
            let kind_id = bound_signal(&html, "资源类型");
            for (query, kind, total, marker) in [
                (
                    "page-model-09",
                    "model",
                    1,
                    format!("data-group-model=\"{last_model}\""),
                ),
                (
                    "shared",
                    "",
                    1,
                    format!("data-group-route=\"{}\"", route.id),
                ),
                ("", "route", 1, format!("data-group-route=\"{}\"", route.id)),
                ("not-found", "", 0, "没有匹配的模型或路由".to_owned()),
            ] {
                let mut filtered = signals.clone();
                filtered[&page_id]["v"] = "1".into();
                filtered[&query_id] = query.into();
                filtered[&kind_id] = kind.into();
                let filtered = client
                    .post(&url)
                    .header("x-topcoat-runtime", "true")
                    .json(&serde_json::json!({"signals":filtered}))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                assert!(filtered.contains(&format!(
                    "显示 {}–{total} 条，共 {total} 条",
                    usize::from(total > 0)
                )));
                assert!(filtered.contains(&marker));
            }
        }
        signals[&page_id]["v"] = "1".into();
        signals[&size_id] = "15".into();
        let resized = client
            .post(&url)
            .header("x-topcoat-runtime", "true")
            .json(&serde_json::json!({"signals":signals}))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            group_panel(&resized, panel).contains(&format!("显示 1–{total} 条，共 {total} 条")),
            "{path}"
        );
    }
    for id in ["not-a-number", "0", "-1", "9999"] {
        assert_eq!(
            client
                .get(format!("{base}/ui/groups/{id}"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        for suffix in ["models", "keys"] {
            assert_eq!(
                client
                    .get(format!("{base}/ui/groups/{id}/{suffix}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
        }
    }
}

fn pagination_signals(html: &str) -> serde_json::Map<String, serde_json::Value> {
    html.split("<!--::topcoat::signal(")
        .skip(1)
        .map(|part| {
            let declaration = part
                .split(")-->")
                .next()
                .unwrap()
                .replace("&quot;", "\"")
                .replace("&amp;", "&");
            let value: serde_json::Value = serde_json::from_str(&declaration).unwrap();
            (value["id"].as_str().unwrap().to_owned(), value["v"].clone())
        })
        .collect()
}

fn bound_signal(html: &str, label: &str) -> String {
    hydrated_signal(&element_attribute(
        html,
        &format!("aria-label=\"{label}\""),
        "data-topcoat-bind:value",
    ))
}

fn click_signal(html: &str, marker: &str) -> String {
    hydrated_signal(&element_attribute(html, marker, "data-topcoat-on:click"))
}

fn element_attribute(html: &str, marker: &str, attribute: &str) -> String {
    let position = html.find(marker).unwrap();
    let start = html[..position].rfind('<').unwrap();
    html[start..]
        .split(&format!("{attribute}=\""))
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .replace("&quot;", "\"")
}

fn hydrated_signal(binding: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(
        binding
            .split("cx.hydrate(")
            .nth(1)
            .unwrap_or_else(|| panic!("missing signal hydration: {binding}"))
            .split("))")
            .next()
            .unwrap(),
    )
    .unwrap();
    value["id"].as_str().unwrap().to_owned()
}

fn group_panel<'a>(html: &'a str, panel: &str) -> &'a str {
    match panel {
        "models" => html
            .split("id=\"group-models-panel\"")
            .nth(1)
            .unwrap()
            .split("id=\"group-keys-panel\"")
            .next()
            .unwrap(),
        "keys" => html.split("id=\"group-keys-panel\"").nth(1).unwrap(),
        _ => html,
    }
}

async fn procedure(
    client: &reqwest::Client,
    base: &str,
    endpoint: &str,
    cookie: Option<&str>,
    fields: &[(&str, &str)],
) -> reqwest::Response {
    let payload = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish();
    let mut request = client
        .post(format!("{base}/ui/_topcoat/runtime/procedures/{endpoint}"))
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&[payload]).unwrap());
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    request.send().await.unwrap()
}
