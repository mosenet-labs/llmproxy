use llmproxy_core::protocol::{MessagesAuth, Protocol};
use llmproxy_store::ModelProbeTarget;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ModelCandidate {
    pub id: String,
    pub input_price_per_million: Option<String>,
    pub output_price_per_million: Option<String>,
}

pub(crate) async fn query_models(
    target: ModelProbeTarget,
) -> std::result::Result<Vec<ModelCandidate>, String> {
    let openrouter = target.host.eq_ignore_ascii_case("openrouter.ai")
        || target.host.to_ascii_lowercase().ends_with(".openrouter.ai");
    let scheme = if target.tls { "https" } else { "http" };
    let mut url = reqwest::Url::parse(&format!(
        "{scheme}://{}:{}{}",
        target.host, target.port, target.path
    ))
    .map_err(|_| "无效的模型列表地址".to_owned())?;
    if target.protocol == Protocol::Gemini {
        url.query_pairs_mut().append_pair("pageSize", "1000");
    }
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
        Protocol::Gemini => request.header("x-goog-api-key", &target.secret),
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
        if body.len() + chunk.len() > 8 * 1024 * 1024 {
            return Err("模型列表响应超过 8 MiB".to_owned());
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "上游没有返回有效 JSON".to_owned())?;
    let field = if target.protocol == Protocol::Gemini {
        "models"
    } else {
        "data"
    };
    let data = value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("上游响应缺少 {field} 模型列表"))?;
    Ok(data
        .iter()
        .filter_map(|model| {
            if target.protocol == Protocol::Gemini {
                parse_gemini_candidate(model)
            } else {
                parse_candidate(model, openrouter)
            }
        })
        .take(5000)
        .collect())
}

fn parse_gemini_candidate(model: &serde_json::Value) -> Option<ModelCandidate> {
    let id = model.get("name")?.as_str()?.strip_prefix("models/")?;
    if id.is_empty() || id.len() > 200 || id.contains('/') {
        return None;
    }
    if !model
        .get("supportedGenerationMethods")?
        .as_array()?
        .iter()
        .any(|method| method == "generateContent")
    {
        return None;
    }
    Some(ModelCandidate {
        id: id.to_owned(),
        input_price_per_million: None,
        output_price_per_million: None,
    })
}

fn parse_candidate(model: &serde_json::Value, openrouter: bool) -> Option<ModelCandidate> {
    let id = model.get("id")?.as_str()?;
    if id.is_empty() || id.len() > 200 {
        return None;
    }
    let prices = openrouter.then(|| model.get("pricing")).flatten();
    let input = prices
        .and_then(|price| price.get("prompt"))
        .and_then(per_million);
    let output = prices
        .and_then(|price| price.get("completion"))
        .and_then(per_million);
    Some(ModelCandidate {
        id: id.to_owned(),
        input_price_per_million: input.clone().filter(|_| output.is_some()),
        output_price_per_million: output.filter(|_| input.is_some()),
    })
}

fn per_million(value: &serde_json::Value) -> Option<String> {
    let price = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))?;
    let token_price = Decimal::from_str_exact(&price)
        .or_else(|_| Decimal::from_scientific(&price))
        .ok()?;
    let amount = token_price.checked_mul(Decimal::from(1_000_000))?;
    if amount.is_sign_negative() || amount > Decimal::from(1_000_000) {
        return None;
    }
    Some(amount.normalize().to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_candidate, parse_gemini_candidate, per_million};
    use serde_json::json;

    #[test]
    fn converts_openrouter_token_prices_to_million_token_prices() {
        assert_eq!(per_million(&json!("0.00000015")).as_deref(), Some("0.15"));
        assert_eq!(per_million(&json!("0")).as_deref(), Some("0"));
        assert_eq!(
            per_million(&json!("0.0000000000001")).as_deref(),
            Some("0.0000001")
        );
        assert!(per_million(&json!("unknown")).is_none());
    }

    #[test]
    fn only_uses_complete_openrouter_pricing() {
        let model =
            json!({"id":"vendor/model", "pricing":{"prompt":"0.00000015", "completion":"0"}});
        let candidate = parse_candidate(&model, true).unwrap();
        assert_eq!(candidate.input_price_per_million.as_deref(), Some("0.15"));
        assert_eq!(candidate.output_price_per_million.as_deref(), Some("0"));
        assert!(
            parse_candidate(&model, false)
                .unwrap()
                .input_price_per_million
                .is_none()
        );
        let partial = json!({"id":"vendor/model", "pricing":{"prompt":"0.00000015"}});
        assert!(
            parse_candidate(&partial, true)
                .unwrap()
                .input_price_per_million
                .is_none()
        );
    }

    #[test]
    fn parses_gemini_generation_models() {
        let model =
            json!({"name":"models/gemini-test","supportedGenerationMethods":["generateContent"]});
        assert_eq!(parse_gemini_candidate(&model).unwrap().id, "gemini-test");
        assert!(parse_gemini_candidate(&json!({"name":"models/embedding-test","supportedGenerationMethods":["embedContent"]})).is_none());
    }
}
