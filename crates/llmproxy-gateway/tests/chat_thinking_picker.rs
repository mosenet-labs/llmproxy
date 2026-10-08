#[allow(dead_code)]
mod nonstream;
mod support;

use llmproxy_core::{
    conversation::Selection,
    ir::request::controls::Reasoning,
    protocol::Protocol,
    thinking::{Choice, Config, Support},
};
use llmproxy_store::{MessagesAuth, ModelMappingInput, ProviderInput, ProviderPaths};
use nonstream::{Database, MASTER_KEY};
use serde_json::{Value, json};
use support::Gateway;

fn declaration(html: &str) -> Value {
    let raw = html
        .split("<!--::topcoat::signal(")
        .nth(1)
        .unwrap()
        .split_once(")-->")
        .unwrap()
        .0;
    serde_json::from_str(&raw.replace("&quot;", "\"")).unwrap()
}

#[tokio::test]
async fn thinking_picker_does_not_reuse_another_conversations_client_value() {
    let database = Database::new().await;
    let provider = database
        .store
        .create(ProviderInput {
            name: "thinking".into(),
            paths: ProviderPaths::single(Protocol::OpenAiResponses),
            models_protocol: Protocol::OpenAiResponses,
            host: "127.0.0.1".into(),
            port: 12345,
            tls: false,
            api_key: "unused-test-key".into(),
            enabled: true,
            models_path: "/models".into(),
            anthropic_version: None,
            messages_auth: MessagesAuth::ApiKey,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
        })
        .await
        .unwrap();
    let model = database
        .store
        .create_model(ModelMappingInput {
            alias: "thinking-test".into(),
            provider_id: provider.id,
            upstream_model_id: "test-model".into(),
            protocols: vec![Protocol::OpenAiResponses],
            reference_price: None,
            thinking: Config {
                support: Support::Switchable,
                enabled: Reasoning {
                    effort: Some("medium".into()),
                    ..Default::default()
                },
            },
        })
        .await
        .unwrap();
    let selection = Selection {
        model_id: model.id.to_string(),
        protocol: Protocol::OpenAiResponses,
    };
    let enabled = "1".repeat(32);
    let default = "2".repeat(32);
    for key in [&enabled, &default] {
        database
            .store
            .create_chat_conversation(key, &selection)
            .await
            .unwrap();
    }
    database
        .store
        .select_chat(&enabled, &selection, Choice::Enabled)
        .await
        .unwrap();
    let gateway = Gateway::database(&database.url, MASTER_KEY);
    let client = reqwest::Client::new();
    let base = format!("http://{}/ui", gateway.address);
    let form = client
        .get(format!("{base}/providers/form"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = form
        .split("name=\"csrf\"")
        .nth(1)
        .unwrap()
        .split("value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    for key in [&enabled, &default] {
        let response = client
            .post(format!("{base}/_topcoat/runtime/procedures/open-chat"))
            .json(&json!([csrf, key]))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
    let mut signals = json!({});
    // 浏览器带着之前控件的值切换会话；当前会话的保存值应决定回显。
    for (key, expected) in [
        (&enabled, "enabled"),
        (&default, "default"),
        (&enabled, "enabled"),
    ] {
        let signal =
            |id: u8, value: Value| json!({"t":"Signal","id":format!("{id:032x}"),"v":value});
        let response = client
            .post(format!(
                "{base}/_topcoat/runtime/shards/chat-thinking-picker"
            ))
            .header("x-topcoat-identity", "_glY_FmvJupFutmO6b-Ysw")
            .json(&json!({"args":[
                signal(1, json!(model.id.to_string())), signal(2, json!(key)),
                signal(3, json!({"t":"usize","bits":64,"v":"0"})),
                signal(4, json!(false)), signal(5, json!(""))
            ],"signals":signals}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert!(status.is_success(), "{body}");
        let choice = declaration(&body);
        assert_eq!(choice["v"], expected, "conversation {key}");
        signals[choice["id"].as_str().unwrap()] = choice["v"].clone();
    }
}
