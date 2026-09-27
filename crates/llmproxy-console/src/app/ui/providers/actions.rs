use super::*;

#[procedure("/ui/_topcoat/runtime/procedures/preview-models")]
pub async fn preview_models(
    cx: &Cx,
    csrf: String,
    id: String,
    name: String,
    openai_chat: bool,
    openai_responses: bool,
    anthropic_messages: bool,
    upstream_url: String,
    api_key: String,
    models_path: String,
    models_protocol: String,
    anthropic_version: String,
) -> Result<Outcome> {
    let input = ProviderForm {
        csrf,
        id: (!id.is_empty()).then_some(id),
        name,
        openai_chat,
        openai_responses,
        anthropic_messages,
        upstream_url,
        api_key,
        models_path,
        models_protocol,
        anthropic_version,
        ..ProviderForm::default()
    };
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let name = if input.name.trim().is_empty() {
        "当前配置".to_owned()
    } else {
        input.name.clone()
    };
    let id = input
        .id
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok());
    let result = match input.preview_input() {
        Ok(candidate) => match state.store.preview_target(id, candidate).await {
            Ok(target) => query_models(target).await,
            Err(error) => Err(error.to_string()),
        },
        Err(error) => Err(error),
    };
    state.telemetry.provider_operation(
        "preview_models",
        id,
        Some(&name),
        result.as_ref().err().map(|_| &StoreError::Internal),
    );
    Ok(probe_message(&name, result))
}

fn probe_message(name: &str, result: std::result::Result<Vec<String>, String>) -> Outcome {
    result
        .map(|models| {
            if models.is_empty() {
                format!("「{name}」模型探测成功，上游未返回模型 ID")
            } else {
                format!(
                    "「{name}」探测到 {} 个模型，示例：{}",
                    models.len(),
                    models
                        .iter()
                        .take(10)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("、")
                )
            }
        })
        .map_err(|error| format!("「{name}」模型探测失败：{error}"))
}

#[procedure("/ui/_topcoat/runtime/procedures/save-provider")]
pub async fn save_provider(cx: &Cx, payload: String) -> Result<Outcome> {
    let Form(input) = Form::<ProviderForm>::from_bytes(payload.as_bytes())?;
    save_input(cx, input).await
}

pub(super) async fn save_input(cx: &Cx, mut input: ProviderForm) -> Result<Outcome> {
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let store = &state.store;
    let record_id = input.id.as_deref().and_then(|id| id.parse::<i64>().ok());
    let result: std::result::Result<ProviderView, StoreError> = async {
        let status = ProbeStatus::parse(&input.models_probe_status)?;
        let provider = input.input().map_err(StoreError::Validation)?;
        let saved = match input.id.as_deref() {
            Some(id) => match (
                id.parse::<i64>(),
                input.version.as_deref().unwrap_or("").parse::<u64>(),
            ) {
                (Ok(id), Ok(version)) => {
                    store
                        .update_with_probe_status(id, version, provider, Some(status))
                        .await
                }
                _ => Err(StoreError::Validation(
                    "表单版本无效，请重新打开编辑页".to_owned(),
                )),
            },
            None => store.create_with_probe_status(provider, status).await,
        }?;
        Ok(saved)
    }
    .await;
    input.api_key.clear();
    state.telemetry.provider_operation(
        if input.id.is_some() {
            "update"
        } else {
            "create"
        },
        result
            .as_ref()
            .ok()
            .map(|provider| provider.id)
            .or(record_id),
        Some(
            result
                .as_ref()
                .map(|provider| provider.name.as_str())
                .unwrap_or(&input.name),
        ),
        result.as_ref().err(),
    );
    Ok(result
        .map(|provider| {
            let name = provider.name;
            format!(
                "「{name}」{}",
                if input.id.is_some() {
                    "已保存"
                } else {
                    "已创建"
                }
            )
        })
        .map_err(|error| format!("「{}」保存失败：{error}", input.name)))
}

#[procedure("/ui/_topcoat/runtime/procedures/provider-action")]
pub async fn provider_action(
    cx: &Cx,
    csrf: String,
    id: String,
    version: String,
    action: String,
) -> Result<Outcome> {
    action_input(
        cx,
        ActionForm {
            csrf,
            id: id.parse()?,
            version: version.parse()?,
            action,
        },
    )
    .await
}

pub(super) async fn action_input(cx: &Cx, input: ActionForm) -> Result<Outcome> {
    check_csrf(cx, &input.csrf)?;
    let state = app_context::<AppState>(cx);
    let store = &state.store;
    let (action, label, completed) = match input.action.as_str() {
        "enable" => ("enable", "启用", "已启用"),
        "disable" => ("disable", "停用", "已停用"),
        "delete" => ("delete", "删除", "已删除"),
        _ => return Err(topcoat::router::error::bad_request("无效的操作").into()),
    };
    let name = match store.get(input.id).await {
        Ok(provider) => provider.name,
        Err(error) => {
            state
                .telemetry
                .provider_operation(action, Some(input.id), None, Some(&error));
            return Err(error.into());
        }
    };
    let result = match input.action.as_str() {
        "enable" => store
            .set_enabled(input.id, input.version, true)
            .await
            .map(|_| ()),
        "disable" => store
            .set_enabled(input.id, input.version, false)
            .await
            .map(|_| ()),
        "delete" => store.delete(input.id, input.version).await.map(|_| ()),
        _ => unreachable!(),
    };
    state
        .telemetry
        .provider_operation(action, Some(input.id), Some(&name), result.as_ref().err());
    Ok(result
        .map(|_| format!("「{name}」{completed}"))
        .map_err(|error| format!("「{name}」{label}失败：{error}")))
}
