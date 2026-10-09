use super::*;

#[derive(Deserialize)]
struct ModelForm {
    thinking_support: String,
    thinking_mode: String,
    thinking_effort: String,
    thinking_budget: String,

    id: Option<String>,
    version: Option<String>,
    alias: String,
    provider_id: String,
    upstream_model_id: String,
    chat: bool,
    responses: bool,
    messages: bool,
    gemini: bool,
    input_price_per_million: String,
    output_price_per_million: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct DraftModel {
    model_id: String,
    alias: String,
    chat: bool,
    responses: bool,
    messages: bool,
    #[serde(default)]
    gemini: bool,
    input_price_per_million: Option<String>,
    output_price_per_million: Option<String>,
}

#[derive(Deserialize)]
struct BatchModelForm {
    csrf: String,
    provider_id: String,
    models: Vec<DraftModel>,
}

#[procedure("/ui/_topcoat/runtime/procedures/load-model-candidates")]
pub async fn load_model_candidates(
    cx: &Cx,
    provider_id: String,
) -> Result<std::result::Result<Vec<ModelCandidate>, String>> {
    let result: std::result::Result<Vec<ModelCandidate>, String> = async {
        let id = provider_id
            .parse::<i64>()
            .map_err(|_| "请选择 Provider".to_owned())?;
        let target = app_context::<AppState>(cx)
            .store
            .clone()
            .probe_enabled_target(id)
            .await
            .map_err(|error| error.to_string())?;
        query_models(target).await
    }
    .await;
    Ok(result)
}

#[procedure("/ui/_topcoat/runtime/procedures/save-models")]
pub async fn save_models(
    cx: &Cx,
    csrf: String,
    provider_id: String,
    models_json: String,
) -> Result<Outcome> {
    let input = BatchModelForm {
        csrf,
        provider_id,
        models: serde_json::from_str(&models_json)
            .map_err(|_| topcoat::router::error::bad_request("无效的模型表单"))?,
    };
    check_csrf(cx, &input.csrf)?;
    let store = &app_context::<AppState>(cx).store.clone();
    let result: std::result::Result<String, StoreError> = async {
        let provider_id = input
            .provider_id
            .parse::<i64>()
            .map_err(|_| StoreError::Validation("请选择 Provider".into()))?;
        if input.models.is_empty() {
            return Err(StoreError::Validation("请至少选择一个模型".into()));
        }
        let provider = store.get(provider_id).await?;
        let mut mappings = Vec::with_capacity(input.models.len());
        for model in input.models {
            let mut protocols = Vec::new();
            if model.chat {
                protocols.push(Protocol::OpenAiChat);
            }
            if model.responses {
                protocols.push(Protocol::OpenAiResponses);
            }
            if model.messages {
                protocols.push(Protocol::AnthropicMessages);
            }
            if model.gemini {
                protocols.push(Protocol::Gemini);
            }
            mappings.push(ModelMappingInput {
                thinking: Default::default(),
                alias: if model.alias.trim().is_empty() {
                    format!("{}/{}", provider.name, model.model_id)
                } else {
                    model.alias
                },
                provider_id,
                upstream_model_id: model.model_id,
                protocols,
                reference_price: model
                    .input_price_per_million
                    .zip(model.output_price_per_million)
                    .map(|(input_per_million, output_per_million)| ModelPrice {
                        input_per_million,
                        output_per_million,
                    }),
            });
        }
        let count = store.create_models(mappings).await?.len();
        Ok(format!("已导入 {count} 个模型"))
    }
    .await;
    Ok(result.map_err(|error| error.to_string()))
}

#[procedure("/ui/_topcoat/runtime/procedures/save-model")]
pub async fn save_model(cx: &Cx, csrf: String, model_json: String) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let mut input: ModelForm = serde_json::from_str(&model_json)
        .map_err(|_| topcoat::router::error::bad_request("无效的模型表单"))?;
    input.id = input.id.filter(|id| !id.is_empty());
    input.version = input.version.filter(|v| !v.is_empty());
    let store = &app_context::<AppState>(cx).store.clone();
    let result: std::result::Result<String, StoreError> = async {
        let provider_id = input
            .provider_id
            .parse::<i64>()
            .map_err(|_| StoreError::Validation("请选择 Provider".into()))?;
        let mut protocols = Vec::new();
        if input.chat {
            protocols.push(Protocol::OpenAiChat);
        }
        if input.responses {
            protocols.push(Protocol::OpenAiResponses);
        }
        if input.messages {
            protocols.push(Protocol::AnthropicMessages);
        }
        if input.gemini {
            protocols.push(Protocol::Gemini);
        }
        let provider = store.get(provider_id).await?;
        let alias = if input.alias.trim().is_empty() {
            format!("{}/{}", provider.name, input.upstream_model_id)
        } else {
            input.alias.clone()
        };
        let candidate = ModelMappingInput {
            thinking: thinking::parse(
                &input.thinking_support,
                &input.thinking_mode,
                &input.thinking_effort,
                &input.thinking_budget,
            )?,
            alias,
            provider_id,
            upstream_model_id: input.upstream_model_id.clone(),
            protocols,
            reference_price: (!input.input_price_per_million.is_empty()
                && !input.output_price_per_million.is_empty())
            .then(|| ModelPrice {
                input_per_million: input.input_price_per_million.clone(),
                output_per_million: input.output_price_per_million.clone(),
            }),
        };
        let existing = if let Some(id) = input.id.as_deref() {
            Some(
                store
                    .get_model(
                        id.parse()
                            .map_err(|_| StoreError::Validation("模型 ID 无效".into()))?,
                    )
                    .await?,
            )
        } else {
            None
        };
        let saved = if let Some(old) = existing {
            let version = input
                .version
                .as_deref()
                .unwrap_or("")
                .parse()
                .map_err(|_| StoreError::Validation("模型版本无效".into()))?;
            store.update_model(old.id, version, candidate).await?
        } else {
            store.create_model(candidate).await?
        };
        Ok(format!(
            "「{}」{}",
            saved.alias,
            if input.id.is_some() {
                "已保存"
            } else {
                "已创建"
            }
        ))
    }
    .await;
    Ok(result.map_err(|error| error.to_string()))
}

#[procedure("/ui/_topcoat/runtime/procedures/delete-model")]
pub async fn delete_model(cx: &Cx, csrf: String, id: String, version: String) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let store = &app_context::<AppState>(cx).store.clone();
    let id = id.parse::<i64>()?;
    let version = version.parse::<u64>()?;
    let model = store.get_model(id).await?;
    Ok(store
        .delete_model(id, version)
        .await
        .map(|_| format!("「{}」已删除", model.alias))
        .map_err(|error| error.to_string()))
}

#[procedure("/ui/_topcoat/runtime/procedures/probe-saved-model")]
pub async fn probe_saved_model(
    cx: &Cx,
    csrf: String,
    id: String,
    protocol: String,
    max_output_tokens: String,
) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let result: std::result::Result<String, String> = async {
        let id = id.parse::<i64>().map_err(|_| "模型 ID 无效".to_owned())?;
        let protocol = match protocol.as_str() {
            "openai_chat" => Protocol::OpenAiChat,
            "openai_responses" => Protocol::OpenAiResponses,
            "anthropic_messages" => Protocol::AnthropicMessages,
            "gemini" => Protocol::Gemini,
            _ => return Err("请选择有效的探测协议".to_owned()),
        };
        let max_output_tokens = max_output_tokens
            .parse::<u32>()
            .ok()
            .filter(|value| (1..=1024).contains(value))
            .ok_or_else(|| "输出上限须为 1–1024 token".to_owned())?;
        let state = app_context::<AppState>(cx);
        let (alias, probe) = state.health.probe(id, protocol, max_output_tokens).await?;
        let label = format!("「{}」{}", alias, protocol_label(protocol));
        let thinking_note = if probe.thinking_mode == ThinkingMode::Low {
            "（低思考模式，仍会消耗思考 token）"
        } else {
            ""
        };
        match probe.verdict {
            Verdict::Available => {
                let usage = probe.usage.map_or(String::new(), |usage| {
                    format!("，输入 {} / 输出 {} token", usage.input, usage.output)
                });
                Ok(format!("{label} 探测可用{usage}{thinking_note}"))
            }
            Verdict::Unavailable => Err(format!("{label} 上游明确报告模型不存在{thinking_note}")),
            Verdict::Inconclusive => {
                let reason = match probe.reason {
                    Some(Reason::Authentication) => "上游鉴权失败",
                    Some(Reason::RateLimited) => "上游限流",
                    Some(Reason::Timeout) => "请求超时",
                    Some(Reason::Connection) => "无法连接上游",
                    Some(Reason::InvalidRequest) => "上游不接受探测参数，请检查协议或调整输出上限",
                    Some(Reason::InvalidResponse) => "上游响应格式不符",
                    Some(Reason::ThinkingStillEnabled) => "上游仍返回思考内容，无法确认已关闭思考",
                    _ => "上游请求失败",
                };
                Err(format!("{label} 无法判定：{reason}{thinking_note}"))
            }
        }
    }
    .await;
    Ok(result)
}

#[topcoat::view::component]
pub(super) async fn model_delete(
    cx: &Cx,
    model: &ModelMappingView,
    csrf: &str,
    success: &Signal<String>,
    failure: &Signal<String>,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let id = format!("delete-model-{}", model.id);
    let title = format!(
        "确认删除「{}」？所有组都将移除此模型。仅需移出某个组，请到资源组管理。",
        model.alias
    );
    let trigger = popconfirm_trigger_attributes(cx, &id);
    let model_id = model.id.to_string();
    let version = model.version.to_string();
    let unavailable: Outcome = Err("删除请求失败，请刷新后重试".into());
    Ok(view! {
        <button class=(class!(TEXT_LINK, "text-[#cf1322]!")) type="button" (trigger)>
            "删除"
        </button>
        popconfirm(
            id: id.as_str(),
            title: title.as_str(),
            language: UiLanguage::ChineseSimplified,
            <button
                class="gr-button gr-button-danger"
                type="button"
                @click=$(async |_event: Event| {
                    let result = raw!(
                        "await Promise.resolve(${delete_model}.call(${csrf}, ${model_id}, ${version})).catch(() => ${unavailable})",
                        unavailable.clone(),
                    );
                    if result.is_ok() {
                        success.set(result.unwrap());
                        refresh.increment();
                    } else {
                        failure.set(result.unwrap_err());
                    }
                })
            >
                "确认删除"
            </button>
        )
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/remove-draft-model")]
pub async fn remove_draft_model(selection: String, model_id: String) -> Result<String> {
    let mut selected: Vec<String> = serde_json::from_str(&selection).unwrap_or_default();
    selected.retain(|id| id != &model_id);
    Ok(serde_json::to_string(&selected)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/add-draft-model")]
pub async fn add_draft_model(selection: String, model_id: String) -> Result<String> {
    let mut selected: Vec<String> = serde_json::from_str(&selection).unwrap_or_default();
    let model_id = model_id.trim();
    if !model_id.is_empty() && !selected.iter().any(|id| id == model_id) {
        selected.push(model_id.to_owned());
    }
    Ok(serde_json::to_string(&selected)?)
}
