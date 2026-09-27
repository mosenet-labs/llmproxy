use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::ModelProbeTarget;

pub(crate) const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

pub(crate) async fn query_models(
    target: ModelProbeTarget,
) -> std::result::Result<Vec<String>, String> {
    let scheme = if target.tls { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}{}", target.host, target.port, target.path);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| "无法创建模型探测客户端".to_owned())?;
    let request = client.get(url).header("accept", "application/json");
    let request = match target.protocol {
        Protocol::OpenAiChat | Protocol::OpenAiResponses => request.bearer_auth(&target.secret),
        Protocol::AnthropicMessages => {
            let request = match target.messages_auth {
                MessagesAuth::ApiKey => request.header("x-api-key", &target.secret),
                MessagesAuth::Bearer => request.bearer_auth(&target.secret),
            };
            request.header(
                "anthropic-version",
                target
                    .anthropic_version
                    .as_deref()
                    .unwrap_or(DEFAULT_ANTHROPIC_VERSION),
            )
        }
    };
    let mut response = request
        .send()
        .await
        .map_err(|_| "无法连接上游模型列表接口".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("上游返回 HTTP {}", response.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "读取上游响应失败".to_owned())?
    {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("模型列表响应超过 1 MiB".to_owned());
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "上游没有返回有效 JSON".to_owned())?;
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "上游响应缺少 data 模型列表".to_owned())?;
    Ok(data
        .iter()
        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
        .filter(|id| !id.is_empty() && id.len() <= 200)
        .take(5000)
        .map(str::to_owned)
        .collect())
}
