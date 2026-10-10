// Topcoat shards and procedures receive their signal fields as separate arguments.
#![expect(clippy::too_many_arguments)]

use super::table::page_numbers;
use std::collections::HashSet;

use llmproxy_core::protocol::Protocol;
use llmproxy_probe::{Reason, ThinkingMode, Verdict};
use llmproxy_store::{
    ModelMappingInput, ModelMappingView, ModelPrice, PricePlanView, ProviderView, StoreError,
};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{href, page},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{Attributes, View, attributes, class, view},
};
use topcoat_ant_design::icons::CLOSE_OUTLINED;
use topcoat_ant_design::{
    FormFieldConfig, NativeDialogConfig, NotificationTone, SearchOption, TagTone, UiLanguage,
    anchored_menu_trigger_attributes, data_table, form_field, native_dialog,
    native_dialog_close_attributes, notification, popconfirm, popconfirm_trigger_attributes,
    search_multi_select, table_page_size_select, table_pagination, tag,
};

use crate::app::{
    check_csrf,
    model_catalog::{ModelCandidate, query_models},
};

mod actions;
mod editor;
mod health;
mod pricing;
mod thinking;
use health::{HealthEditor, health_dialog, health_trigger};
pub(crate) use health::{model_health, save_health_check};

use actions::model_delete;
pub(crate) use actions::{
    add_draft_model, delete_model, load_model_candidates, probe_saved_model, remove_draft_model,
    save_model, save_models,
};
pub(crate) use editor::model_draft;
use editor::{Editor, editor_trigger, model_editor};
use pricing::{PriceEditor, price_editor, price_label, price_lines, price_trigger};
pub(crate) use pricing::{
    add_price_rule, add_price_window, deepseek_peak_windows, preview_price_band, price_matrix_rows,
    price_peak_windows, price_rule_rows, remove_price_rule, remove_price_window, save_price_plan,
};

const BUTTON: &str = "inline-flex h-9 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-control-border bg-white px-4 text-sm font-medium leading-5 text-heading hover:border-primary-hover hover:text-primary";
const PRIMARY: &str = "border-primary! bg-primary! text-white! shadow-sm hover:border-primary-hover! hover:bg-primary-hover!";
const TEXT_LINK: &str = "border-0 bg-transparent p-0 text-sm leading-[22px] whitespace-nowrap text-primary hover:text-primary-hover";

type Outcome = std::result::Result<String, String>;

fn protocol_label(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::OpenAiChat => "Chat",
        Protocol::OpenAiResponses => "Responses",
        Protocol::AnthropicMessages => "Messages",
        Protocol::Gemini => "Gemini",
    }
}

#[page]
pub async fn models(cx: &Cx) -> Result<impl View> {
    use topcoat::view::ViewExt;
    if !crate::app::store(cx).can_manage_resources() {
        return Ok(super::resource_catalog::member_catalog(cx, false)
            .await?
            .boxed());
    }

    let refresh = signal(cx, || 0.0);
    let success = signal(cx, String::new);
    let failure = signal(cx, String::new);
    Ok(view! {
        notification(
            message: &success,
            title: "操作成功",
            tone: NotificationTone::Success,
            language: UiLanguage::ChineseSimplified
        )
        notification(
            message: &failure,
            title: "操作失败",
            tone: NotificationTone::Error,
            language: UiLanguage::ChineseSimplified
        )
        model_workspace(
            revision: $(refresh.get()),
            success: $(success),
            failure: $(failure),
            refresh: $(refresh)
        )
    }
    .boxed())
}

#[shard("/ui/_topcoat/runtime/shards/model-workspace")]
pub async fn model_workspace(
    cx: &Cx,
    revision: f64,
    success: Signal<String>,
    failure: Signal<String>,
    refresh: Signal<f64>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let draft_query = signal(cx, String::new);
    let draft_protocol = signal(cx, String::new);
    let draft_provider = signal(cx, String::new);
    let applied_query = signal(cx, String::new);
    let applied_protocol = signal(cx, String::new);
    let applied_provider = signal(cx, String::new);
    let page = signal(cx, || 1usize);
    let page_size = signal(cx, || "10".to_owned());
    let all = crate::app::store(cx).list_all_models().await?;
    let price_plans = crate::app::store(cx).list_current_price_plans().await?;
    let all_providers = crate::app::store(cx).list().await?;
    let providers: Vec<_> = all_providers
        .iter()
        .filter(|provider| provider.enabled)
        .cloned()
        .collect();
    let filter_providers: Vec<_> = all_providers
        .iter()
        .filter(|provider| all.iter().any(|model| model.provider_id == provider.id))
        .cloned()
        .collect();
    let query = applied_query.get();
    let protocol = applied_protocol.get();
    let provider = applied_provider.get();
    let needle = query.trim().to_lowercase();
    let filtered: Vec<_> = all
        .iter()
        .filter(|m| {
            (needle.is_empty()
                || m.alias.to_lowercase().contains(&needle)
                || m.provider_name.to_lowercase().contains(&needle)
                || m.upstream_model_id.to_lowercase().contains(&needle))
                && (protocol.is_empty() || m.protocols.iter().any(|kind| kind.as_str() == protocol))
                && (provider.is_empty() || m.provider_id.to_string() == provider)
        })
        .cloned()
        .collect();
    let size = match page_size.get().as_str() {
        "15" => 15,
        "30" => 30,
        "50" => 50,
        "100" => 100,
        _ => 10,
    };
    let total = filtered.len();
    let page_count = total.div_ceil(size).max(1);
    let current_page = page.get().clamp(1, page_count);
    let start = (current_page - 1) * size;
    let end = (start + size).min(total);
    let page_models: Vec<_> = filtered.iter().skip(start).take(size).cloned().collect();
    let summary = format!(
        "显示 {}–{} 条，共 {total} 条",
        if total == 0 { 0 } else { start + 1 },
        end
    );
    let previous_page = current_page.saturating_sub(1).max(1);
    let next_page = current_page.saturating_add(1).min(page_count);
    let pages = page_numbers(current_page, page_count);
    let mut health_enabled = HashSet::new();
    let mut health_states = std::collections::HashMap::new();
    for model in &page_models {
        let checks = crate::app::store(cx).model_health_checks(model.id).await?;
        if checks.iter().any(|check| check.config.enabled) {
            health_enabled.insert(model.id);
        }
        let (label, tone) = health::list_status(model.provider_enabled, &checks);
        let detail = checks
            .iter()
            .filter(|check| check.config.enabled)
            .map(|check| {
                format!(
                    "{}：{}",
                    protocol_label(check.protocol),
                    health::status_label(check.status)
                )
            })
            .collect::<Vec<_>>()
            .join("；");
        health_states.insert(model.id, (label, tone, detail));
    }
    let health_editor = HealthEditor::new(cx);
    let editor = Editor::new(cx);
    let price_editor_state = PriceEditor::new(cx);
    let create = editor_trigger(cx, &editor, None, None, None);
    let csrf = crate::app::auth::csrf_token(cx);
    let reset = attributes! {
        cx =>
        @click=$(|_event: Event| {
            draft_query.set("".to_owned());
            draft_protocol.set("".to_owned());
            draft_provider.set("".to_owned());
            applied_query.set("".to_owned());
            applied_protocol.set("".to_owned());
            applied_provider.set("".to_owned());
            page.set(1);
            refresh.increment();
        })
    };
    Ok(view! {
        model_editor(
            editor: &editor,
            providers: &providers,
            all_models: &all,
            csrf: csrf.as_str(),
            success: &success,
            refresh: &refresh
        )
        health_dialog(editor: &health_editor, refresh: &refresh, success: &success)
        price_editor(
            editor: &price_editor_state,
            csrf: csrf.as_str(),
            success: &success,
            refresh: &refresh
        )
        <section class=(super::providers::PAGE_HEADING)>
            <div>
                <h1 class="sr-only">"Models"</h1>
                <p>
                    "从 Provider 导入模型到系统，再在资源组中选择需要开放的模型。导入不会自动加入任何组。"
                </p>
            </div>
            <button
                class=(class!(BUTTON, PRIMARY))
                type="button"
                (create)
                :disabled=(providers.is_empty())
            >
                "＋ 新建模型"
            </button>
        </section>
        <section
            class="overflow-visible rounded-lg border border-border bg-white shadow-xs"
            aria-label="模型列表"
        >
            <div
                class="flex flex-wrap items-center justify-between gap-4 px-6 py-5 max-[640px]:px-4"
            >
                <h2 class="m-0 text-base font-semibold">
                    "模型列表"
                    <span
                        class="ml-2 rounded bg-surface px-2 text-[13px] font-normal text-secondary"
                    >
                        (all.len())
                    </span>
                </h2>
                <div class="flex min-w-0 flex-wrap items-center gap-4">
                <a
                    class="text-[13px] text-primary hover:underline"
                    (topcoat::runtime::link_attrs(
                        cx, crate::app::scoped_href(cx, "/ui/groups"),
                        topcoat::runtime::prefetch_mode(cx),
                    ))
                >
                    "管理组资源 →"
                </a>
                </div>
            </div>
            <form
                class="flex flex-wrap items-center gap-3 border-t border-border px-6 py-4 max-[640px]:px-4 [&_select]:h-9 [&_select]:min-w-[150px] [&_select]:text-sm max-[640px]:[&_select]:min-w-0 max-[640px]:[&_select]:flex-1"
                method="get"
                action="/ui/models"
                role="search"
                @submit=$(|event: Event| {
                    event.prevent_default();
                    applied_query.set(draft_query.get());
                    applied_protocol.set(draft_protocol.get());
                    applied_provider.set(draft_provider.get());
                    page.set(1);
                    refresh.increment();
                })
            >
                <input
                    class="h-9 w-[300px] max-w-full rounded-md border border-control-border px-3 text-sm focus:border-primary max-[640px]:w-full"
                    type="search"
                    name="q"
                    aria-label="搜索模型"
                    placeholder="搜索标识、上游模型或 Provider"
                    :value=$(draft_query.get())
                    @input=$(|event: Event| draft_query.set(event.target.value))
                >
                <select
                    name="protocol"
                    aria-label="按协议筛选模型"
                    :value=$(draft_protocol.get())
                    @change=$(|event: Event| draft_protocol.set(event.target.value))
                >
                    <option value="">"全部协议"</option>
                    for kind in [
                        Protocol::OpenAiChat,
                        Protocol::OpenAiResponses,
                        Protocol::AnthropicMessages,
                        Protocol::Gemini,
                    ] {
                        <option
                            value=(kind.as_str())
                            selected=(draft_protocol.get_untracked() == kind.as_str())
                        >
                            (protocol_label(kind))
                        </option>
                    }
                </select>
                <select
                    name="provider"
                    aria-label="按 Provider 筛选模型"
                    :value=$(draft_provider.get())
                    @change=$(|event: Event| draft_provider.set(event.target.value))
                >
                    <option value="">"全部 Provider"</option>
                    for option in &filter_providers {
                        <option
                            value=(option.id.to_string())
                            selected=(draft_provider.get_untracked()
                                == option.id.to_string())
                        >
                            (option.name.as_str())
                        </option>
                    }
                </select>
                <button class=(BUTTON) type="submit">"查询"</button>
                if !query.is_empty() || !protocol.is_empty() || !provider.is_empty() {
                    <button
                        class=(class!(TEXT_LINK, "px-1"))
                        type="button"
                        (reset.clone())
                    >
                        "重置"
                    </button>
                }
            </form>
            if all.is_empty() {
                <div class="border-t border-border px-6 py-14 text-center">
                    <h3 class="m-0 text-base font-medium">"尚未添加模型"</h3>
                    <p class="mt-2 mb-0 text-sm text-secondary">
                        "先启用 Provider，再选择或手动添加模型。"
                    </p>
                    if providers.is_empty() {
                        <a
                            class=(class!(BUTTON, PRIMARY, "mt-5"))
                            href=(crate::app::scoped_href(cx, href!(super::providers::list) .resolve(cx)))
                        >
                            "管理 Providers"
                        </a>
                    } else {
                        <button
                            class=(class!(BUTTON, PRIMARY, "mt-5"))
                            type="button"
                            (editor_trigger(cx, &editor, None, None, None))
                        >
                            "新建模型"
                        </button>
                    }
                </div>
            } else if filtered.is_empty() {
                <div
                    class="border-t border-border px-6 py-12 text-center text-sm text-secondary"
                >
                    "没有找到匹配的模型"
                </div>
            } else {
                data_table(
                    label: "模型列表",
                    attrs: attributes! {
                        class="min-w-[960px] [&_th]:px-5! [&_td]:h-auto! [&_td]:px-5! [&_td]:py-2!"
                    },
                    <thead>
                        <tr>
                            <th>"模型标识"</th>
                            <th>"上游模型 ID"</th>
                            <th>"Provider"</th>
                            <th>"协议"</th>
                            <th>"参考价格"</th>
                            <th>"状态"</th>
                            <th>"探活"</th>
                            <th class="text-right!">"操作"</th>
                        </tr>
                    </thead>
                    <tbody>
                        #[key(model.id)]
                        for model in &page_models {
                            let plan: Option<&PricePlanView> = price_plans
                                .iter()
                                .find(
                                    |plan| plan.provider_id == model.provider_id
                                            && plan.upstream_model_id == model.upstream_model_id,
                                );
                            let alias_count = all
                                .iter()
                                .filter(
                                    |other| other.provider_id == model.provider_id
                                            && other.upstream_model_id == model.upstream_model_id,
                                )
                                .count();
                            let old_price_needs_review = plan.is_none()
                                && all
                                    .iter()
                                    .any(
                                        |other| other.provider_id == model.provider_id
                                                && other.upstream_model_id == model.upstream_model_id
                                                && other.reference_price.is_some(),
                                    );
                            let label = price_label(plan, old_price_needs_review);
                            let price_lines = plan.and_then(price_lines);
                            <tr>
                                <td>
                                    <button
                                        class="border-0 bg-transparent p-0 text-left text-sm font-medium text-heading hover:text-primary"
                                        type="button"
                                        (editor_trigger(
                                            cx,
                                            &editor,
                                            Some(model),
                                            providers.iter().find(|p| p.id == model.provider_id),
                                            plan,
                                        ))
                                    >
                                        (model.alias.as_str())
                                    </button>
                                </td>
                                <td class="text-sm text-secondary">
                                    (model.upstream_model_id.as_str())
                                </td>
                                <td class="text-sm text-heading">
                                    (model.provider_name.as_str())
                                </td>
                                <td>
                                    <div class="flex flex-wrap gap-1">
                                        for protocol in &model.protocols {
                                            tag(tone: TagTone::Default, (protocol_label(*protocol)))
                                        }
                                    </div>
                                </td>
                                <td>
                                    <button
                                        class="border-0 bg-transparent p-0 text-left text-xs leading-4 font-medium tabular-nums text-primary hover:underline"
                                        type="button"
                                        aria-label=(label.as_str())
                                        (price_trigger(
                                            cx,
                                            &price_editor_state,
                                            model,
                                            plan,
                                            alias_count,
                                        ))
                                    >
                                        if let Some(lines) = price_lines {
                                            for line in lines {
                                                <span class="block whitespace-nowrap">(line)</span>
                                            }
                                        } else {
                                            <span class="text-[13px] leading-5">(label)</span>
                                        }
                                    </button>
                                </td>
                                <td>
                                    if let Some((label, tone, detail)) = health_states.get(
                                        &model.id,
                                    ) {
                                        <span title=(detail.as_str())>
                                            tag(tone: *tone, (*label))
                                        </span>
                                    }
                                </td>
                                <td>
                                    tag(
                                        tone: if health_enabled.contains(&model.id) {
                                            TagTone::Success
                                        } else {
                                            TagTone::Default
                                        },
                                        (if health_enabled.contains(&model.id) {
                                            "已配置"
                                        } else {
                                            "未配置"
                                        })
                                    )
                                </td>
                                <td>
                                    <div
                                        class="flex items-center justify-end gap-3 whitespace-nowrap"
                                    >
                                        <button
                                            class=(TEXT_LINK)
                                            type="button"
                                            (editor_trigger(
                                                cx,
                                                &editor,
                                                Some(model),
                                                providers.iter().find(|p| p.id == model.provider_id),
                                                plan,
                                            ))
                                        >
                                            "编辑"
                                        </button>
                                        <button
                                            class=(TEXT_LINK)
                                            type="button"
                                            (health_trigger(cx, &health_editor, model))
                                        >
                                            "探活配置"
                                        </button>
                                        model_delete(
                                            model: model,
                                            csrf: csrf.as_str(),
                                            success: &success,
                                            failure: &failure,
                                            refresh: &refresh
                                        )
                                    </div>
                                </td>
                            </tr>
                        }
                    </tbody>
                )
            }
            if !all.is_empty() {
                table_pagination(
                    summary: summary.as_str(),
                    label: "模型列表分页",
                    attrs: attributes! { class="border-t border-border [&_nav]:flex-wrap" },
                    <div class="shrink-0">
                    table_page_size_select(
                        id: "models-page-size",
                        label: "每页记录数",
                        attrs: attributes! {
                            cx =>
                            :value=$(page_size.get())
                            @change=$(|event: Event| {
                                page_size.set(event.target.value);
                                page.set(1);
                            })
                        },
                        <option value="10">"10"</option>
                        <option value="15">"15"</option>
                        <option value="30">"30"</option>
                        <option value="50">"50"</option>
                        <option value="100">"100"</option>
                    )
                    </div>
                    <button
                        type="button"
                        :disabled=(current_page == 1)
                        @click=$(|_event: Event| page.set(previous_page))
                    >
                        "上一页"
                    </button>
                    for number in pages {
                        if number == 0 {
                            <span aria-hidden="true">"…"</span>
                        } else {
                            <button
                                type="button"
                                aria-label=(format!("第 {number} 页"))
                                aria-current=(if number == current_page {
                                    Some("page")
                                } else {
                                    None
                                })
                                @click=$(|_event: Event| page.set(number))
                            >
                                (number)
                            </button>
                        }
                    }
                    <button
                        type="button"
                        :disabled=(current_page == page_count)
                        @click=$(|_event: Event| page.set(next_page))
                    >
                        "下一页"
                    </button>
                )
            }
        </section>
    })
}

#[shard("/ui/_topcoat/runtime/shards/model-candidates")]
pub async fn model_candidates(cx: &Cx, provider_id: String, open: bool) -> Result<impl View> {
    crate::app::request_connection(cx);
    let result: std::result::Result<Vec<ModelCandidate>, String> =
        if !open || provider_id.is_empty() {
            Ok(Vec::new())
        } else {
            match provider_id.parse::<i64>() {
                Ok(id) => match crate::app::store(cx).probe_enabled_target(id).await {
                    Ok(target) => query_models(target).await,
                    Err(error) => Err(error.to_string()),
                },
                Err(_) => Err("请选择 Provider".to_owned()),
            }
        };
    Ok(view! {
        <datalist id="model-candidates">
            if let Ok(candidates) = &result {
                for candidate in candidates {
                    <option value=(candidate.id.as_str())></option>
                }
            }
        </datalist>
        if open && !provider_id.is_empty() {
            <p class="mt-2 mb-0 text-[13px] text-secondary" role="status">
                (match &result {
                    Ok(candidates) => format!(
                        "探测到 {} 个模型，可搜索选择或直接填写模型 ID。",
                        candidates.len(),
                    ),
                    Err(error) => format!(
                        "模型探测失败：{error}；仍可直接填写模型 ID。",
                    ),
                })
            </p>
        }
    })
}
