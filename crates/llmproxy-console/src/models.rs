use std::collections::HashSet;
use std::time::Duration;

use llmproxy_core::protocol::Protocol;
use llmproxy_probe::{InferenceProbeTarget, Reason, ThinkingMode, Verdict};
use llmproxy_store::{ModelMappingInput, ModelMappingView, ProviderView, StoreError};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::page,
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, class, view},
};
use topcoat_ant_design::icons::CLOSE_OUTLINED;
use topcoat_ant_design::{
    FormFieldConfig, NativeDialogConfig, NotificationTone, SearchOption, TagTone, UiLanguage,
    data_table, form_field, native_dialog, native_dialog_close_attributes, notification,
    popconfirm, popconfirm_trigger_attributes, search_multi_select, tag,
};

use crate::app::{AppState, check_csrf, query_models};

const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-control-border bg-white px-4 text-sm font-medium leading-5 text-heading hover:border-primary-hover hover:text-primary";
const PRIMARY: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const TEXT_LINK: &str = "border-0 bg-transparent p-0 text-sm leading-[22px] whitespace-nowrap text-primary hover:text-primary-hover";

type Outcome = std::result::Result<String, String>;

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Responses",
        Protocol::AnthropicMessages => "Messages",
    }
}

#[page("/ui/models")]
pub async fn models(cx: &Cx) -> Result<impl View> {
    let refresh = signal(cx, || 0.0);
    let search = signal(cx, String::new);
    let success = signal(cx, String::new);
    let failure = signal(cx, String::new);
    Ok(view! {
        notification(message: &success, title: "操作成功", tone: NotificationTone::Success, language: UiLanguage::ChineseSimplified)
        notification(message: &failure, title: "操作失败", tone: NotificationTone::Error, language: UiLanguage::ChineseSimplified)
        model_workspace(revision: $(refresh.get()), query: $(search.get()), search: $(search), success: $(success), failure: $(failure), refresh: $(refresh))
    })
}

struct Editor {
    open: Signal<bool>,
    busy: Signal<bool>,
    error: Signal<String>,
    id: Signal<String>,
    version: Signal<String>,
    alias: Signal<String>,
    alias_edited: Signal<bool>,
    provider_id: Signal<String>,
    provider_name: Signal<String>,
    provider_query: Signal<String>,
    provider_menu_open: Signal<bool>,
    model_id: Signal<String>,
    model_search: Signal<String>,
    model_menu_open: Signal<bool>,
    selected_models: Signal<String>,
    draft_models: Signal<String>,
    draft_revision: Signal<f64>,
    candidates: Signal<String>,
    candidate_error: Signal<String>,
    candidate_busy: Signal<bool>,
    chat: Signal<bool>,
    responses: Signal<bool>,
    messages: Signal<bool>,
    supports_chat: Signal<bool>,
    supports_responses: Signal<bool>,
    supports_messages: Signal<bool>,
    probe_protocol: Signal<String>,
    probe_tokens: Signal<String>,
    probe_busy: Signal<bool>,
    probe_success: Signal<String>,
    probe_failure: Signal<String>,
    probe_chat: Signal<bool>,
    probe_responses: Signal<bool>,
    probe_messages: Signal<bool>,
}

impl Editor {
    fn new(cx: &Cx) -> Self {
        Self {
            open: signal(cx, || false),
            busy: signal(cx, || false),
            error: signal(cx, String::new),
            id: signal(cx, String::new),
            version: signal(cx, String::new),
            alias: signal(cx, String::new),
            alias_edited: signal(cx, || false),
            provider_id: signal(cx, String::new),
            provider_name: signal(cx, String::new),
            provider_query: signal(cx, String::new),
            provider_menu_open: signal(cx, || false),
            model_id: signal(cx, String::new),
            model_search: signal(cx, String::new),
            model_menu_open: signal(cx, || false),
            selected_models: signal(cx, || "[]".to_owned()),
            draft_models: signal(cx, || "[]".to_owned()),
            draft_revision: signal(cx, || 0.0),
            candidates: signal(cx, || "[]".to_owned()),
            candidate_error: signal(cx, String::new),
            candidate_busy: signal(cx, || false),
            chat: signal(cx, || false),
            responses: signal(cx, || false),
            messages: signal(cx, || false),
            supports_chat: signal(cx, || false),
            supports_responses: signal(cx, || false),
            supports_messages: signal(cx, || false),
            probe_protocol: signal(cx, String::new),
            probe_tokens: signal(cx, || "1".to_owned()),
            probe_busy: signal(cx, || false),
            probe_success: signal(cx, String::new),
            probe_failure: signal(cx, String::new),
            probe_chat: signal(cx, || false),
            probe_responses: signal(cx, || false),
            probe_messages: signal(cx, || false),
        }
    }
}

fn editor_trigger(
    cx: &Cx,
    editor: &Editor,
    model: Option<&ModelMappingView>,
    provider: Option<&ProviderView>,
) -> Attributes {
    let (id, version, alias, provider_id, model_id, chat, responses, messages) =
        if let Some(model) = model {
            (
                model.id.to_string(),
                model.version.to_string(),
                model.alias.clone(),
                model.provider_id.to_string(),
                model.upstream_model_id.clone(),
                model.protocols.contains(&Protocol::OpenAiChat),
                model.protocols.contains(&Protocol::OpenAiResponses),
                model.protocols.contains(&Protocol::AnthropicMessages),
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                false,
                false,
                false,
            )
        };
    let Editor {
        open,
        error,
        id: selected_id,
        version: selected_version,
        alias: selected_alias,
        alias_edited,
        provider_id: selected_provider,
        provider_name: selected_provider_name,
        provider_query,
        provider_menu_open,
        model_id: selected_model,
        model_search,
        model_menu_open,
        selected_models,
        draft_models,
        draft_revision,
        candidates,
        candidate_error,
        candidate_busy,
        chat: selected_chat,
        responses: selected_responses,
        messages: selected_messages,
        supports_chat,
        supports_responses,
        supports_messages,
        probe_protocol,
        probe_tokens,
        probe_busy,
        probe_success,
        probe_failure,
        probe_chat,
        probe_responses,
        probe_messages,
        ..
    } = editor;
    let supported_chat = provider.is_some_and(|provider| provider.paths.openai_chat.is_some());
    let supported_responses =
        provider.is_some_and(|provider| provider.paths.openai_responses.is_some());
    let supported_messages =
        provider.is_some_and(|provider| provider.paths.anthropic_messages.is_some());
    let provider_name = provider
        .map_or("", |provider| provider.name.as_str())
        .to_owned();
    let initial_probe_protocol = if chat {
        Protocol::OpenAiChat.as_str()
    } else if responses {
        Protocol::OpenAiResponses.as_str()
    } else if messages {
        Protocol::AnthropicMessages.as_str()
    } else {
        ""
    };
    attributes! { cx => aria-haspopup="dialog" aria-controls="model-dialog" @click=$(|_event: Event| {
        selected_id.set(id.to_owned());
        selected_version.set(version.to_owned());
        selected_alias.set(alias.to_owned());
        alias_edited.set(!id.is_empty());
        selected_provider.set(provider_id.to_owned());
        selected_provider_name.set(provider_name.to_owned());
        provider_query.set(provider_name.to_owned());
        provider_menu_open.set(false);
        selected_model.set(model_id.to_owned());
        model_search.set("".to_owned());
        model_menu_open.set(false);
        selected_models.set("[]".to_owned());
        draft_models.set("[]".to_owned());
        draft_revision.increment();
        candidates.set("[]".to_owned());
        candidate_error.set("".to_owned());
        candidate_busy.set(false);
        selected_chat.set(chat);
        selected_responses.set(responses);
        selected_messages.set(messages);
        supports_chat.set(supported_chat);
        supports_responses.set(supported_responses);
        supports_messages.set(supported_messages);
        probe_protocol.set(initial_probe_protocol.to_owned());
        probe_tokens.set("1".to_owned());
        probe_busy.set(false);
        probe_success.set("".to_owned());
        probe_failure.set("".to_owned());
        probe_chat.set(chat);
        probe_responses.set(responses);
        probe_messages.set(messages);
        error.set("".to_owned());
        open.set(true);
    }) }
}

#[shard("/ui/_topcoat/runtime/shards/model-workspace")]
pub async fn model_workspace(
    cx: &Cx,
    revision: f64,
    query: String,
    search: Signal<String>,
    success: Signal<String>,
    failure: Signal<String>,
    refresh: Signal<f64>,
) -> Result<impl View> {
    let _ = revision;
    let state = app_context::<AppState>(cx);
    let all = state.store.list_models().await?;
    let providers: Vec<_> = state
        .store
        .list()
        .await?
        .into_iter()
        .filter(|p| p.enabled)
        .collect();
    let needle = query.trim().to_lowercase();
    let filtered: Vec<_> = all
        .iter()
        .filter(|m| {
            needle.is_empty()
                || m.alias.to_lowercase().contains(&needle)
                || m.provider_name.to_lowercase().contains(&needle)
                || m.upstream_model_id.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    let editor = Editor::new(cx);
    let create = editor_trigger(cx, &editor, None, None);
    let csrf = state.csrf.clone();
    Ok(view! {
        model_editor(editor: &editor, providers: &providers, csrf: csrf.as_str(), success: &success, refresh: &refresh)
        <section class="mb-6 flex min-h-20 items-center justify-between gap-6 max-[640px]:flex-col max-[640px]:items-start max-[640px]:gap-4">
            <div><h1 class="m-0 text-[28px] font-semibold leading-[1.35] text-heading max-[640px]:text-2xl">"Models"</h1><p class="mt-2 mb-0 text-sm text-secondary">"选择上游模型，设置客户端别名与可用协议。"</p></div>
            <button class=(class!(BUTTON, PRIMARY)) type="button" (create) :disabled=(providers.is_empty())>"＋ 新建模型"</button>
        </section>
        <section class="overflow-visible rounded-lg border border-border bg-white shadow-xs" aria-label="模型列表">
            <div class="flex items-center justify-between gap-4 px-6 py-5 max-[640px]:px-4"><h2 class="m-0 text-base font-semibold">"模型列表"<span class="ml-2 rounded bg-surface px-2 text-[13px] font-normal text-secondary">(all.len())</span></h2><span class="text-[13px] text-secondary">"按协议与别名匹配请求"</span></div>
            <div class="border-t border-border px-6 py-4 max-[640px]:px-4"><input class="h-9 w-[320px] max-w-full rounded-md border border-control-border px-3 text-sm focus:border-primary" type="search" aria-label="搜索模型" placeholder="搜索别名、上游模型或 Provider" :value=$(search.get()) @input=$(|event: Event| search.set(event.target.value))></div>
            if all.is_empty() {
                <div class="border-t border-border px-6 py-14 text-center"><h3 class="m-0 text-base font-medium">"尚未添加模型"</h3><p class="mt-2 mb-0 text-sm text-secondary">"先启用 Provider，再从其模型列表选择模型。"</p>if providers.is_empty() { <a class=(class!(BUTTON, PRIMARY, "mt-5")) href="/ui/providers">"管理 Providers"</a> } else { <button class=(class!(BUTTON, PRIMARY, "mt-5")) type="button" (editor_trigger(cx, &editor, None, None))>"新建模型"</button> }</div>
            } else if filtered.is_empty() {
                <div class="border-t border-border px-6 py-12 text-center text-sm text-secondary">"没有找到匹配的模型"</div>
            } else {
                data_table(label: "模型列表", attrs: attributes! { class="min-w-[780px] [&_th]:px-6! [&_td]:px-6! [&_td]:py-4!" },
                    <thead><tr><th>"模型别名"</th><th>"上游模型 ID"</th><th>"Provider"</th><th>"协议"</th><th>"状态"</th><th class="text-right!">"操作"</th></tr></thead>
                    <tbody>#[key(model.id)] for model in &filtered {
                        <tr>
                            <td><button class="border-0 bg-transparent p-0 text-left text-sm font-medium text-heading hover:text-primary" type="button" (editor_trigger(cx, &editor, Some(model), providers.iter().find(|p| p.id == model.provider_id)))>(model.alias.as_str())</button></td>
                            <td class="text-sm text-secondary">(model.upstream_model_id.as_str())</td>
                            <td class="text-sm text-heading">(model.provider_name.as_str())</td>
                            <td><div class="flex flex-wrap gap-1">for protocol in &model.protocols { tag(tone: TagTone::Default, (protocol_label(*protocol))) }</div></td>
                            <td>tag(tone: if model.provider_enabled { TagTone::Success } else { TagTone::Warning }, (if model.provider_enabled { "可用" } else { "Provider 已停用" }))</td>
                            <td><div class="flex items-center justify-end gap-3 whitespace-nowrap"><button class=(TEXT_LINK) type="button" (editor_trigger(cx, &editor, Some(model), providers.iter().find(|p| p.id == model.provider_id)))>"编辑"</button>model_delete(model: model, csrf: csrf.as_str(), success: &success, failure: &failure, refresh: &refresh)</div></td>
                        </tr>
                    }</tbody>
                )
            }
        </section>
    })
}

#[shard("/ui/_topcoat/runtime/shards/model-candidates")]
pub async fn model_candidates(cx: &Cx, provider_id: String, open: bool) -> Result<impl View> {
    let state = app_context::<AppState>(cx);
    let result: std::result::Result<Vec<String>, String> = if !open || provider_id.is_empty() {
        Ok(Vec::new())
    } else {
        match provider_id.parse::<i64>() {
            Ok(id) => match state.store.probe_enabled_target(id).await {
                Ok(target) => query_models(target).await,
                Err(error) => Err(error.to_string()),
            },
            Err(_) => Err("请选择 Provider".to_owned()),
        }
    };
    Ok(view! {
        <datalist id="model-candidates">if let Ok(candidates) = &result { for candidate in candidates { <option value=(candidate.as_str())></option> } }</datalist>
        if open && !provider_id.is_empty() { <p class="mt-2 mb-0 text-[13px] text-secondary" role="status">(match &result { Ok(candidates) => format!("探测到 {} 个模型，可输入关键词搜索并选择。", candidates.len()), Err(error) => format!("模型探测失败：{error}") })</p> }
    })
}

#[derive(Deserialize)]
struct ModelForm {
    csrf: String,
    id: Option<String>,
    version: Option<String>,
    alias: String,
    provider_id: String,
    upstream_model_id: String,
    chat: bool,
    responses: bool,
    messages: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct DraftModel {
    model_id: String,
    alias: String,
    chat: bool,
    responses: bool,
    messages: bool,
}

#[derive(Deserialize)]
struct BatchModelForm {
    csrf: String,
    provider_id: String,
    models: Vec<DraftModel>,
}

#[procedure("/ui/_topcoat/runtime/procedures/load-model-candidates")]
pub async fn load_model_candidates(cx: &Cx, provider_id: String) -> Result<Outcome> {
    let result: std::result::Result<String, String> = async {
        let id = provider_id
            .parse::<i64>()
            .map_err(|_| "请选择 Provider".to_owned())?;
        let target = app_context::<AppState>(cx)
            .store
            .probe_enabled_target(id)
            .await
            .map_err(|error| error.to_string())?;
        let candidates = query_models(target).await?;
        serde_json::to_string(&candidates).map_err(|_| "无法读取模型列表".to_owned())
    }
    .await;
    Ok(result)
}

#[procedure("/ui/_topcoat/runtime/procedures/save-models")]
pub async fn save_models(cx: &Cx, payload: String) -> Result<Outcome> {
    let input: BatchModelForm = serde_json::from_str(&payload)
        .map_err(|_| topcoat::router::error::bad_request("无效的模型表单"))?;
    check_csrf(cx, &input.csrf)?;
    let store = &app_context::<AppState>(cx).store;
    let result: std::result::Result<String, StoreError> = async {
        let provider_id = input
            .provider_id
            .parse::<i64>()
            .map_err(|_| StoreError::Validation("请选择 Provider".into()))?;
        if input.models.is_empty() {
            return Err(StoreError::Validation("请至少选择一个模型".into()));
        }
        let provider = store.get(provider_id).await?;
        let target = store.probe_enabled_target(provider_id).await?;
        let candidates: HashSet<_> = query_models(target)
            .await
            .map_err(StoreError::Validation)?
            .into_iter()
            .collect();
        let mut mappings = Vec::with_capacity(input.models.len());
        for model in input.models {
            if !candidates.contains(&model.model_id) {
                return Err(StoreError::Validation(format!(
                    "「{}」不在 Provider 探测结果中",
                    model.model_id
                )));
            }
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
            mappings.push(ModelMappingInput {
                alias: if model.alias.trim().is_empty() {
                    format!("{}/{}", provider.name, model.model_id)
                } else {
                    model.alias
                },
                provider_id,
                upstream_model_id: model.model_id,
                protocols,
            });
        }
        let count = store.create_models(mappings).await?.len();
        Ok(format!("已导入 {count} 个模型"))
    }
    .await;
    Ok(result.map_err(|error| error.to_string()))
}

#[procedure("/ui/_topcoat/runtime/procedures/save-model")]
pub async fn save_model(cx: &Cx, payload: String) -> Result<Outcome> {
    let input: ModelForm = serde_json::from_str(&payload)
        .map_err(|_| topcoat::router::error::bad_request("无效的模型表单"))?;
    check_csrf(cx, &input.csrf)?;
    let store = &app_context::<AppState>(cx).store;
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
        let provider = store.get(provider_id).await?;
        let alias = if input.alias.trim().is_empty() {
            format!("{}/{}", provider.name, input.upstream_model_id)
        } else {
            input.alias.clone()
        };
        let candidate = ModelMappingInput {
            alias,
            provider_id,
            upstream_model_id: input.upstream_model_id.clone(),
            protocols,
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
        if existing.as_ref().is_none_or(|old| {
            old.provider_id != provider_id || old.upstream_model_id != input.upstream_model_id
        }) {
            let target = store.probe_enabled_target(provider_id).await?;
            let candidates = query_models(target).await.map_err(StoreError::Validation)?;
            if !candidates.contains(&input.upstream_model_id) {
                return Err(StoreError::Validation(
                    "所选模型不在 Provider 探测结果中".into(),
                ));
            }
        }
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
    let store = &app_context::<AppState>(cx).store;
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
            _ => return Err("请选择有效的探测协议".to_owned()),
        };
        let max_output_tokens = max_output_tokens
            .parse::<u32>()
            .ok()
            .filter(|value| (1..=1024).contains(value))
            .ok_or_else(|| "输出上限须为 1–1024 token".to_owned())?;
        let state = app_context::<AppState>(cx);
        let route = state
            .store
            .load_model_route(id, protocol)
            .await
            .map_err(|error| error.to_string())?;
        let provider = route
            .provider
            .ok_or_else(|| "Provider 已停用，无法探测".to_owned())?;
        let provider_id = provider.id;
        let scheme = if provider.tls { "https" } else { "http" };
        let url = format!(
            "{scheme}://{}:{}{}",
            provider.host, provider.port, provider.upstream_path
        )
        .parse()
        .map_err(|_| "Provider 上游地址无效".to_owned())?;
        let target = InferenceProbeTarget {
            url,
            protocol,
            secret: provider.secret,
            anthropic_version: provider.anthropic_version,
            timeout: Duration::from_millis(
                provider
                    .connect_timeout_ms
                    .saturating_add(provider.read_timeout_ms),
            ),
        };
        let probe = state
            .prober
            .probe_model(&target, &route.upstream_model_id, max_output_tokens)
            .await;
        state
            .telemetry
            .model_probe(provider_id, &route.upstream_model_id, protocol, &probe);
        let label = format!("「{}」{}", route.alias, protocol_label(protocol));
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
async fn model_delete(
    cx: &Cx,
    model: &ModelMappingView,
    csrf: &str,
    success: &Signal<String>,
    failure: &Signal<String>,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let id = format!("delete-model-{}", model.id);
    let title = format!("确认删除「{}」？", model.alias);
    let trigger = popconfirm_trigger_attributes(cx, &id);
    let model_id = model.id.to_string();
    let version = model.version.to_string();
    let unavailable: Outcome = Err("删除请求失败，请刷新后重试".into());
    Ok(view! {
        <button class=(class!(TEXT_LINK, "text-[#cf1322]!")) type="button" (trigger)>"删除"</button>
        popconfirm(id: id.as_str(), title: title.as_str(), language: UiLanguage::ChineseSimplified,
            <button class="gr-button gr-button-danger" type="button" @click=$(async |_event: Event| {
                let result = raw!("await Promise.resolve(${delete_model}.call(${csrf}, ${model_id}, ${version})).catch(() => ${unavailable})", unavailable.clone());
                if result.is_ok() { success.set(result.unwrap()); refresh.increment(); }
                else { failure.set(result.unwrap_err()); }
            })>"确认删除"</button>
        )
    })
}

#[topcoat::view::component]
async fn provider_search(
    cx: &Cx,
    providers: &[ProviderView],
    editor: &Editor,
) -> Result<impl View> {
    let _ = cx;
    let Editor {
        id: editing_id,
        provider_id,
        provider_name,
        provider_query,
        provider_menu_open,
        model_id,
        alias,
        alias_edited,
        chat,
        responses,
        messages,
        supports_chat,
        supports_responses,
        supports_messages,
        selected_models,
        draft_models,
        draft_revision,
        candidates,
        candidate_error,
        candidate_busy,
        model_search,
        model_menu_open,
        ..
    } = editor;
    let _root_id = "model-provider-select".to_owned();
    let _list_id = "model-provider-options".to_owned();
    let unavailable: Outcome = Err("模型探测请求失败，请重试".into());
    Ok(view! {
        <div id="model-provider-select" class="relative min-w-0" @focusout=$(|_event: Event| {
            let _root_id = _root_id.to_owned();
            raw!("setTimeout(() => { const root = document.getElementById(${_root_id}.dehydrate()); if (root && !root.contains(document.activeElement)) ${provider_menu_open}.set(cx.hydrate(false)); }, 0)", ());
        })>
            <input id="model-provider-input" class="w-full" type="search" role="combobox" aria-label="搜索并选择 Provider" aria-autocomplete="list" aria-controls="model-provider-options" :aria-expanded=$(provider_menu_open.get()) :value=$(provider_query.get()) placeholder="搜索已启用 Provider" autocomplete="off" required=""
                @focus=$(|_event: Event| provider_menu_open.set(true))
                @input=$(|event: Event| {
                    provider_query.set(event.target.value);
                    provider_menu_open.set(true);
                })
                @keydown=$(|_event: Event| {
                    let _list_id = _list_id.to_owned();
                    raw!("{ const key = ${_event}.key.dehydrate(); if (key === 'ArrowDown' || key === 'Enter') { if (key === 'Enter') ${_event}.prevent_default(); const query = ${_event}.target.value.dehydrate().toLowerCase(); const first = [...(document.getElementById(${_list_id}.dehydrate())?.querySelectorAll('button[role=option]') ?? [])].find(option => option.textContent.toLowerCase().includes(query)); if (first) { if (key === 'ArrowDown') ${_event}.prevent_default(); if (key === 'Enter') first.click(); else first.focus(); } } else if (key === 'Escape') { ${provider_menu_open}.set(cx.hydrate(false)); } }", ());
                })>
            <div id="model-provider-options" class="absolute z-30 mt-1 max-h-48 w-full overflow-y-auto rounded-md border border-control-border bg-white p-1 shadow-[0_6px_16px_rgba(0,0,0,0.08)]" role="listbox" aria-label="已启用 Provider" :hidden=$(!provider_menu_open.get())>
                #[key(provider.id)] for provider in providers {
                    let option_id = provider.id.to_string();
                    let option_name = provider.name.clone();
                    let has_chat = provider.paths.openai_chat.is_some();
                    let has_responses = provider.paths.openai_responses.is_some();
                    let has_messages = provider.paths.anthropic_messages.is_some();
                    let only_one = provider.paths.supported().len() == 1;
                    let default_chat = only_one && has_chat;
                    let default_responses = only_one && has_responses;
                    let default_messages = only_one && has_messages;
                    let unavailable = unavailable.clone();
                    <button class="block w-full rounded-md border-0! bg-transparent! px-3 py-2 text-left text-sm leading-5 text-heading shadow-none! hover:bg-primary-soft! focus:bg-primary-soft! focus:outline-none aria-selected:text-primary data-[filtered]:hidden" type="button" role="option" :aria-selected=$(provider_id.get() == option_id) :data-filtered=$(if provider_query.get().is_empty() { false } else { raw!("!${option_name}.dehydrate().toLowerCase().includes(${provider_query}.get().dehydrate().toLowerCase())", false) })
                        @click=$(async |_event: Event| {
                            raw!("if (!(${provider_id}.get().dehydrate() === ${option_id}.dehydrate() || JSON.parse(${draft_models}.get().dehydrate()).length === 0 || window.confirm('切换 Provider 将清空尚未保存的模型，继续吗？'))) return;", ());
                            provider_id.set(option_id.to_owned());
                            provider_name.set(option_name.to_owned());
                            provider_query.set(option_name.to_owned());
                            provider_menu_open.set(false);
                            model_id.set("".to_owned());
                            if !alias_edited.get() { alias.set("".to_owned()); }
                            chat.set(default_chat);
                            responses.set(default_responses);
                            messages.set(default_messages);
                            supports_chat.set(has_chat);
                            supports_responses.set(has_responses);
                            supports_messages.set(has_messages);
                            selected_models.set("[]".to_owned());
                            draft_models.set("[]".to_owned());
                            candidates.set("[]".to_owned());
                            candidate_error.set("".to_owned());
                            model_search.set("".to_owned());
                            model_menu_open.set(false);
                            draft_revision.increment();
                            let creating = editing_id.get().is_empty();
                            if creating { candidate_busy.set(true); }
                            let result = raw!("await Promise.resolve(${creating}.dehydrate() ? ${load_model_candidates}.call(${option_id}) : ${unavailable}).catch(() => ${unavailable})", unavailable.clone());
                            if creating {
                                if provider_id.get() == option_id {
                                    candidate_busy.set(false);
                                    if result.is_ok() { candidates.set(result.unwrap()); draft_revision.increment(); }
                                    else { candidate_error.set(result.unwrap_err()); }
                                }
                            }
                        })>(provider.name.as_str())</button>
                }
            </div>
        </div>
    })
}

#[shard("/ui/_topcoat/runtime/shards/model-draft")]
pub async fn model_draft(
    cx: &Cx,
    revision: f64,
    provider_id: Signal<String>,
    provider_name: Signal<String>,
    candidates: Signal<String>,
    selected: Signal<String>,
    draft: Signal<String>,
    search: Signal<String>,
    menu_open: Signal<bool>,
    supports_chat: Signal<bool>,
    supports_responses: Signal<bool>,
    supports_messages: Signal<bool>,
    refresh: Signal<f64>,
) -> Result<impl View> {
    let _ = cx;
    let _ = revision;
    let available: Vec<String> =
        serde_json::from_str(&candidates.get_untracked()).unwrap_or_default();
    let rows: Vec<DraftModel> = serde_json::from_str(&draft.get_untracked()).unwrap_or_default();
    let mut seen = HashSet::new();
    let options: Vec<SearchOption> = available
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|id| SearchOption {
            value: id.clone(),
            label: id,
        })
        .collect();
    let only_one = [
        supports_chat.get_untracked(),
        supports_responses.get_untracked(),
        supports_messages.get_untracked(),
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count()
        == 1;
    let defaults = serde_json::to_string(&DraftModel {
        model_id: String::new(),
        alias: String::new(),
        chat: only_one && supports_chat.get_untracked(),
        responses: only_one && supports_responses.get_untracked(),
        messages: only_one && supports_messages.get_untracked(),
    })?;
    Ok(view! {
            <div class="min-w-0">
                <label class="mb-2 block text-sm font-semibold text-heading" for="model-select-input">"选择上游模型"<span class="ml-1 text-[#ff4d4f]">"*"</span></label>
                if options.is_empty() {
                    <input id="model-select-input" class="w-full" type="search" placeholder="搜索上游模型" disabled="">
                } else {
                    search_multi_select(id: "model-select", options: &options, selected: &selected, search: &search, open: &menu_open,
                        attrs: attributes! { @selectionchange=$(|_event: Event| {
                            let _defaults = defaults.to_owned();
                            raw!("(() => { const ids = JSON.parse(${selected}.get().dehydrate()); const previous = JSON.parse(${draft}.get().dehydrate()); const base = JSON.parse(${_defaults}.dehydrate()); const name = ${provider_name}.get().dehydrate(); const next = ids.map(id => previous.find(row => row.model_id === id) ?? {...base, model_id:id, alias:name + '/' + id}); ${draft}.set(cx.hydrate(JSON.stringify(next))); })();", ());
                            refresh.increment();
                        }) }
                    )
                }
                <p class="mt-2 mb-0 text-xs text-secondary" role="status">
                    if provider_id.get_untracked().is_empty() { "先选择 Provider，再从探测结果中选择模型。" }
                    else if options.is_empty() { "模型接口未返回可选模型。" }
                    else { (format!("探测到 {} 个模型，可搜索并一次选择多个。", options.len())) }
                </p>
            </div>
            if !rows.is_empty() {
                <div class="col-span-full mt-2">
                    <h3 class="mb-3 text-sm font-semibold text-heading">(format!("待导入模型（{}）", rows.len()))</h3>
                    <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)_minmax(0,1.3fr)_32px] gap-3 border-b border-border pb-2 text-xs font-medium text-secondary max-[760px]:hidden">
                        <span>"模型 ID"</span><span>"客户端别名"</span><span>"支持协议"</span><span class="sr-only">"操作"</span>
                    </div>
                    <div class="divide-y divide-border">
                        #[key(row.model_id)] for row in &rows {
                            let row_id = row.model_id.clone();
                            <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)_minmax(0,1.3fr)_32px] items-start gap-3 py-3 max-[760px]:grid-cols-[minmax(0,1fr)_32px]">
                                <div class="min-w-0 pt-2 text-sm break-all text-heading max-[760px]:col-span-1 max-[760px]:pt-0"><span class="hidden text-xs text-secondary max-[760px]:block">"模型 ID"</span>(row.model_id.as_str())</div>
                                <div class="min-w-0 max-[760px]:col-span-1 max-[760px]:row-start-2"><label class="hidden text-xs text-secondary max-[760px]:mb-1 max-[760px]:block" for=(format!("alias-{}", row.model_id))>"客户端别名"</label><input id=(format!("alias-{}", row.model_id)) class="w-full" type="text" :value=(row.alias.as_str()) maxlength="200" aria-label=(format!("{} 的客户端别名", row.model_id)) @input=$(|_event: Event| {
                                    let _row_id = row_id.to_owned();
                                    raw!("(() => { const rows = JSON.parse(${draft}.get().dehydrate()); const row = rows.find(item => item.model_id === ${_row_id}.dehydrate()); if (row) { row.alias = ${_event}.target.value.dehydrate(); ${draft}.set(cx.hydrate(JSON.stringify(rows))); } })();", ());
                                })></div>
                                <div class="flex min-w-0 flex-wrap gap-x-3 gap-y-1 pt-2 text-xs text-heading max-[760px]:col-span-1 max-[760px]:row-start-3 max-[760px]:pt-0">
                                    <span class="hidden w-full text-xs text-secondary max-[760px]:block">"支持协议"</span>
                                    if supports_chat.get_untracked() { <label class="inline-flex items-center gap-1"><input type="checkbox" :checked=(row.chat) @change=$(|_event: Event| { let _row_id = row_id.to_owned(); raw!("(() => { const rows = JSON.parse(${draft}.get().dehydrate()); const row = rows.find(item => item.model_id === ${_row_id}.dehydrate()); if (row) { row.chat = ${_event}.target.checked.dehydrate(); ${draft}.set(cx.hydrate(JSON.stringify(rows))); } })();", ()); })>"Chat"</label> }
                                    if supports_responses.get_untracked() { <label class="inline-flex items-center gap-1"><input type="checkbox" :checked=(row.responses) @change=$(|_event: Event| { let _row_id = row_id.to_owned(); raw!("(() => { const rows = JSON.parse(${draft}.get().dehydrate()); const row = rows.find(item => item.model_id === ${_row_id}.dehydrate()); if (row) { row.responses = ${_event}.target.checked.dehydrate(); ${draft}.set(cx.hydrate(JSON.stringify(rows))); } })();", ()); })>"Responses"</label> }
                                    if supports_messages.get_untracked() { <label class="inline-flex items-center gap-1"><input type="checkbox" :checked=(row.messages) @change=$(|_event: Event| { let _row_id = row_id.to_owned(); raw!("(() => { const rows = JSON.parse(${draft}.get().dehydrate()); const row = rows.find(item => item.model_id === ${_row_id}.dehydrate()); if (row) { row.messages = ${_event}.target.checked.dehydrate(); ${draft}.set(cx.hydrate(JSON.stringify(rows))); } })();", ()); })>"Messages"</label> }
                                </div>
                                <button class="inline-flex size-8 shrink-0 items-center justify-center self-center rounded-full border border-control-border bg-white p-0 text-muted shadow-sm transition-colors duration-150 hover:border-[#ffccc7] hover:bg-[#fff2f0] hover:text-[#cf1322] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#91caff] max-[760px]:col-start-2 max-[760px]:row-start-1" type="button" aria-label=(format!("移除 {}", row.model_id)) title="移除模型" @click=$(|_event: Event| {
                                    let _row_id = row_id.to_owned();
                                    raw!("(() => { const id = ${_row_id}.dehydrate(); ${selected}.set(cx.hydrate(JSON.stringify(JSON.parse(${selected}.get().dehydrate()).filter(item => item !== id)))); ${draft}.set(cx.hydrate(JSON.stringify(JSON.parse(${draft}.get().dehydrate()).filter(item => item.model_id !== id)))); })();", ());
                                    refresh.increment();
                                })>icon(data: CLOSE_OUTLINED, size: 14)</button>
                            </div>
                        }
                    </div>
                </div>
            }
    })
}

#[topcoat::view::component]
async fn model_editor(
    cx: &Cx,
    editor: &Editor,
    providers: &[ProviderView],
    csrf: &str,
    success: &Signal<String>,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let Editor {
        open,
        busy,
        error,
        id,
        version,
        alias,
        alias_edited,
        provider_id,
        provider_name,
        model_id,
        chat,
        responses,
        messages,
        supports_chat,
        supports_responses,
        supports_messages,
        draft_models,
        draft_revision,
        candidates,
        selected_models,
        model_search,
        model_menu_open,
        candidate_error,
        candidate_busy,
        probe_protocol,
        probe_tokens,
        probe_busy,
        probe_success,
        probe_failure,
        probe_chat,
        probe_responses,
        probe_messages,
        ..
    } = editor;
    let close = native_dialog_close_attributes(cx, "model-dialog");
    let unavailable: Outcome = Err("保存请求失败，请刷新后重试".into());
    let probe_unavailable: Outcome = Err("探测请求失败，请重试".into());
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new("model-dialog", "模型配置"),
            open: Some(open), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="w-[min(960px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:px-6 [&_.gr-native-dialog-header]:py-4" },
            <form class="m-0 flex min-h-0 max-h-[calc(100dvh_-_48px)] flex-col" @submit=$(async |event: Event| {
                event.prevent_default();
                raw!("if (${busy}.get().dehydrate()) return;", ());
                raw!("if (!${provider_id}.get().dehydrate() || (document.getElementById('model-provider-input')?.value ?? '') !== ${provider_name}.get().dehydrate()) { ${error}.set(cx.hydrate('请从下拉列表选择 Provider')); return; }", ());
                busy.set(true);
                error.set("".to_owned());
                let result = raw!("await Promise.resolve(${id}.get().dehydrate() ? ${save_model}.call(cx.hydrate(JSON.stringify({csrf:${csrf}.dehydrate(),id:${id}.get().dehydrate(),version:${version}.get().dehydrate(),alias:${alias}.get().dehydrate(),provider_id:${provider_id}.get().dehydrate(),upstream_model_id:${model_id}.get().dehydrate(),chat:${chat}.get().dehydrate(),responses:${responses}.get().dehydrate(),messages:${messages}.get().dehydrate()}))) : ${save_models}.call(cx.hydrate(JSON.stringify({csrf:${csrf}.dehydrate(),provider_id:${provider_id}.get().dehydrate(),models:JSON.parse(${draft_models}.get().dehydrate())})))).catch(() => ${unavailable})", unavailable.clone());
                busy.set(false);
                if result.is_ok() { open.set(false); success.set(result.unwrap()); refresh.increment(); }
                else { error.set(result.unwrap_err()); }
            })>
                <div class="min-h-[min(320px,calc(100dvh_-_180px))] overflow-y-auto p-6">
                    <div class="mb-5 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]" role="alert" :hidden=$(error.get().is_empty())>$(error.get())</div>
                    <div class="grid grid-cols-2 items-start gap-x-5 gap-y-4 max-[760px]:grid-cols-1">
                        <div class="min-w-0">
                            <label class="mb-2 block text-sm font-semibold" for="model-provider-input">"选择已启用 Provider"<span class="ml-1 text-[#ff4d4f]">"*"</span></label>
                            provider_search(providers: providers, editor: editor)
                        </div>
                        <div class="contents" :hidden=$(!id.get().is_empty())>
                            <p class="mt-8 mb-0 text-sm text-secondary" role="status" :hidden=$(!candidate_busy.get())>"正在探测上游模型…"</p>
                            <p class="mt-8 mb-0 text-sm text-[#cf1322]" role="alert" :hidden=$(candidate_error.get().is_empty())>$(candidate_error.get())</p>
                            <div class="contents" :hidden=$(if candidate_busy.get() { true } else { !candidate_error.get().is_empty() })>
                                model_draft(revision: $(draft_revision.get()), provider_id: provider_id.clone(), provider_name: provider_name.clone(), candidates: candidates.clone(), selected: selected_models.clone(), draft: draft_models.clone(), search: model_search.clone(), menu_open: model_menu_open.clone(), supports_chat: supports_chat.clone(), supports_responses: supports_responses.clone(), supports_messages: supports_messages.clone(), refresh: draft_revision.clone())
                            </div>
                        </div>
                    </div>
                    <div :hidden=$(id.get().is_empty())>
                        <div class="mt-6">form_field(config: FormFieldConfig::new("model-id", "上游模型 ID").required(),
                            <input id="model-id" list="model-candidates" autocomplete="off" :value=$(model_id.get()) @input=$(|event: Event| {
                                model_id.set(event.target.value);
                                if !alias_edited.get() {
                                    alias.set("".to_owned());
                                    if !model_id.get().is_empty() {
                                        alias.set(provider_name.get());
                                        alias.push_str("/");
                                        alias.push_str(model_id.get());
                                    }
                                }
                            }) placeholder="输入关键词搜索探测到的模型" :disabled=$(provider_id.get().is_empty())>
                        )</div>
                        model_candidates(provider_id: $(provider_id.get()), open: $(if id.get().is_empty() { false } else { open.get() }))
                        <div class="mt-6">form_field(config: FormFieldConfig::new("model-alias", "客户端别名"),
                            <input id="model-alias" :value=$(alias.get()) @input=$(|event: Event| { alias.set(event.target.value); alias_edited.set(true); }) placeholder="Provider名称/模型ID" maxlength="200">
                        )</div>
                        <h3 class="mt-6 mb-3 text-sm font-semibold">"选择可用协议（至少一个）"</h3>
                        <div class="flex flex-wrap gap-4 text-sm text-heading">
                            <label class="flex items-center gap-2" :hidden=$(!supports_chat.get())><input type="checkbox" :checked=$(chat.get()) @change=$(|event: Event| chat.set(event.target.checked))>"Chat"</label>
                            <label class="flex items-center gap-2" :hidden=$(!supports_responses.get())><input type="checkbox" :checked=$(responses.get()) @change=$(|event: Event| responses.set(event.target.checked))>"Responses"</label>
                            <label class="flex items-center gap-2" :hidden=$(!supports_messages.get())><input type="checkbox" :checked=$(messages.get()) @change=$(|event: Event| messages.set(event.target.checked))>"Messages"</label>
                        </div>
                        <div class="mt-6 rounded-lg border border-border bg-surface/40 p-4">
                            <h3 class="m-0 text-sm font-semibold text-heading">"可用性探测"</h3>
                            <p class="mt-1 mb-4 text-xs text-secondary">"对已保存的模型发一次极短请求。按已保存的协议和上游路径探测，不修改模型配置。"</p>
                            <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-end gap-3 max-[640px]:grid-cols-1">
                                <label class="min-w-0 text-xs font-medium text-heading">"探测协议"
                                    <select class="mt-2 w-full" :value=$(probe_protocol.get()) @change=$(|event: Event| { probe_protocol.set(event.target.value); probe_success.set("".to_owned()); probe_failure.set("".to_owned()); })>
                                        <option value="openai_chat" :disabled=$(!probe_chat.get()) :hidden=$(!probe_chat.get())>"Chat"</option>
                                        <option value="openai_responses" :disabled=$(!probe_responses.get()) :hidden=$(!probe_responses.get())>"Responses"</option>
                                        <option value="anthropic_messages" :disabled=$(!probe_messages.get()) :hidden=$(!probe_messages.get())>"Messages"</option>
                                    </select>
                                </label>
                                <label class="min-w-0 text-xs font-medium text-heading">"输出上限（token）"
                                    <input class="mt-2 w-full" type="number" min="1" max="1024" step="1" :value=$(probe_tokens.get()) @input=$(|event: Event| { probe_tokens.set(event.target.value); probe_success.set("".to_owned()); probe_failure.set("".to_owned()); })>
                                </label>
                                <button class=(class!(BUTTON, "h-10")) type="button" :disabled=$(probe_busy.get()) @click=$(async |_event: Event| {
                                    raw!("if (${busy}.get().dehydrate() || ${probe_busy}.get().dehydrate()) return;", ());
                                    busy.set(true);
                                    probe_busy.set(true);
                                    probe_success.set("".to_owned());
                                    probe_failure.set("".to_owned());
                                    let result = raw!("await Promise.resolve(${probe_saved_model}.call(${csrf}, ${id}.get(), ${probe_protocol}.get(), ${probe_tokens}.get())).catch(() => ${probe_unavailable})", probe_unavailable.clone());
                                    probe_busy.set(false);
                                    busy.set(false);
                                    if result.is_ok() { probe_success.set(result.unwrap()); }
                                    else { probe_failure.set(result.unwrap_err()); }
                                })>$(if probe_busy.get() { "探测中…" } else { "立即探测" })</button>
                            </div>
                            <p class="mt-3 mb-0 text-sm text-[#389e0d]" role="status" :hidden=$(probe_success.get().is_empty())>$(probe_success.get())</p>
                            <p class="mt-3 mb-0 text-sm text-[#cf1322]" role="status" :hidden=$(probe_failure.get().is_empty())>$(probe_failure.get())</p>
                        </div>
                    </div>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4"><button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button><button class=(class!(BUTTON, PRIMARY)) type="submit" :disabled=$(busy.get())>$(if id.get().is_empty() { "导入模型" } else { "保存模型" })</button></footer>
            </form>
        )
    })
}
