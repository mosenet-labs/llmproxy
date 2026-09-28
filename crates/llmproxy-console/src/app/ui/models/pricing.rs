use super::*;
use chrono::{DateTime, NaiveDateTime, Utc};
use llmproxy_store::{
    PriceConditions, PriceItem, PricePlanInput, PriceRule, PriceSchedule, PriceSource, TimeBand,
    WeeklyPeakWindow,
};

pub(super) struct PriceEditor {
    open: Signal<bool>,
    busy: Signal<bool>,
    error: Signal<String>,
    provider_id: Signal<String>,
    provider_name: Signal<String>,
    model_id: Signal<String>,
    alias_count: Signal<usize>,
    version: Signal<String>,
    source: Signal<String>,
    recorded_at: Signal<String>,
    currency: Signal<String>,
    source_url: Signal<String>,
    effective_at: Signal<String>,
    use_schedule: Signal<bool>,
    timezone: Signal<String>,
    rules_json: Signal<String>,
    windows_json: Signal<String>,
    rules_revision: Signal<f64>,
    windows_revision: Signal<f64>,
}

impl PriceEditor {
    pub(super) fn new(cx: &Cx) -> Self {
        Self {
            open: signal(cx, || false),
            busy: signal(cx, || false),
            error: signal(cx, String::new),
            provider_id: signal(cx, String::new),
            provider_name: signal(cx, String::new),
            model_id: signal(cx, String::new),
            alias_count: signal(cx, || 0usize),
            version: signal(cx, String::new),
            source: signal(cx, String::new),
            recorded_at: signal(cx, String::new),
            currency: signal(cx, || "USD".to_owned()),
            source_url: signal(cx, String::new),
            effective_at: signal(cx, String::new),
            use_schedule: signal(cx, || false),
            timezone: signal(cx, || "UTC".to_owned()),
            rules_json: signal(cx, || "[]".to_owned()),
            windows_json: signal(cx, || "[]".to_owned()),
            rules_revision: signal(cx, || 0.0),
            windows_revision: signal(cx, || 0.0),
        }
    }
}

fn blank_rule() -> PriceRule {
    PriceRule {
        item: PriceItem::Input,
        conditions: PriceConditions::default(),
        unit_price: String::new(),
    }
}

pub(super) fn price_label(plan: Option<&PricePlanView>, old_price_needs_review: bool) -> String {
    let Some(plan) = plan else {
        return if old_price_needs_review {
            "旧价格待确认".into()
        } else {
            "— 设置价格".into()
        };
    };
    if plan.schedule.is_some() || plan.rules.len() != 2 {
        return "按条件计价".into();
    }
    let simple = |item| {
        plan.rules
            .iter()
            .find(|rule| rule.item == item && rule.conditions == PriceConditions::default())
    };
    match (simple(PriceItem::Input), simple(PriceItem::Output)) {
        (Some(input), Some(output)) => {
            let symbol = if plan.currency == "USD" { "$" } else { "" };
            format!(
                "In {symbol}{} · Out {symbol}{} / M · {}",
                input.unit_price, output.unit_price, plan.currency
            )
        }
        _ => "按条件计价".into(),
    }
}

pub(super) fn price_trigger(
    cx: &Cx,
    editor: &PriceEditor,
    model: &ModelMappingView,
    plan: Option<&PricePlanView>,
    alias_count: usize,
) -> Attributes {
    let provider_id = model.provider_id.to_string();
    let provider_name = model.provider_name.clone();
    let model_id = model.upstream_model_id.clone();
    let version = plan.map_or_else(String::new, |plan| format!("#{}", plan.id));
    let source = plan.map_or("尚未设置", |plan| match plan.source {
        PriceSource::Manual => "手动录入",
        PriceSource::Catalog => "模型目录",
        PriceSource::Legacy => "旧价格迁移",
    });
    let recorded_at = plan
        .and_then(|plan| DateTime::<Utc>::from_timestamp(plan.recorded_at, 0))
        .map_or_else(String::new, |time| {
            time.format("%Y-%m-%d %H:%M UTC").to_string()
        });
    let currency = plan.map_or("USD", |plan| plan.currency.as_str()).to_owned();
    let source_url = plan
        .and_then(|plan| plan.source_url.as_deref())
        .unwrap_or("")
        .to_owned();
    let effective_at = plan
        .and_then(|plan| plan.effective_at)
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        .map_or_else(String::new, |time| {
            time.format("%Y-%m-%dT%H:%M").to_string()
        });
    let use_schedule = plan.is_some_and(|plan| plan.schedule.is_some());
    let timezone = plan
        .and_then(|plan| plan.schedule.as_ref())
        .map_or("UTC", |schedule| schedule.timezone.as_str())
        .to_owned();
    let rules = plan.map_or_else(
        || {
            let mut output = blank_rule();
            output.item = PriceItem::Output;
            vec![blank_rule(), output]
        },
        |plan| plan.rules.clone(),
    );
    let rules_json = serde_json::to_string(&rules).expect("price rules are serializable");
    let windows = plan
        .and_then(|plan| plan.schedule.as_ref())
        .map_or_else(Vec::new, |schedule| schedule.peak_windows.clone());
    let windows_json = serde_json::to_string(&windows).expect("price windows are serializable");
    let PriceEditor {
        open,
        busy,
        error,
        provider_id: selected_provider,
        provider_name: selected_name,
        model_id: selected_model,
        alias_count: selected_count,
        version: selected_version,
        source: selected_source,
        recorded_at: selected_recorded_at,
        currency: selected_currency,
        source_url: selected_source_url,
        effective_at: selected_effective_at,
        use_schedule: selected_schedule,
        timezone: selected_timezone,
        rules_json: selected_rules,
        windows_json: selected_windows,
        rules_revision,
        windows_revision,
    } = editor;
    attributes! { cx => aria-haspopup="dialog" aria-controls="price-dialog" @click=$(|_event: Event| {
        selected_provider.set(provider_id.to_owned());
        selected_name.set(provider_name.to_owned());
        selected_model.set(model_id.to_owned());
        selected_count.set(alias_count);
        selected_version.set(version.to_owned());
        selected_source.set(source.to_owned());
        selected_recorded_at.set(recorded_at.to_owned());
        selected_currency.set(currency.to_owned());
        selected_source_url.set(source_url.to_owned());
        selected_effective_at.set(effective_at.to_owned());
        selected_schedule.set(use_schedule);
        selected_timezone.set(timezone.to_owned());
        selected_rules.set(rules_json.to_owned());
        selected_windows.set(windows_json.to_owned());
        rules_revision.increment();
        windows_revision.increment();
        error.set("".to_owned());
        busy.set(false);
        open.set(true);
    }) }
}

#[procedure("/ui/_topcoat/runtime/procedures/add-price-rule")]
pub async fn add_price_rule(json: String) -> Result<String> {
    let mut rules: Vec<PriceRule> = serde_json::from_str(&json)?;
    if rules.len() < 100 {
        rules.push(blank_rule());
    }
    Ok(serde_json::to_string(&rules)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/remove-price-rule")]
pub async fn remove_price_rule(json: String, index: usize) -> Result<String> {
    let mut rules: Vec<PriceRule> = serde_json::from_str(&json)?;
    if index < rules.len() {
        rules.remove(index);
    }
    Ok(serde_json::to_string(&rules)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/add-price-window")]
pub async fn add_price_window(json: String) -> Result<String> {
    let mut windows: Vec<WeeklyPeakWindow> = serde_json::from_str(&json)?;
    if windows.len() < 100 {
        windows.push(WeeklyPeakWindow {
            weekday: 1,
            start: "01:00".into(),
            end: "04:00".into(),
        });
    }
    Ok(serde_json::to_string(&windows)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/remove-price-window")]
pub async fn remove_price_window(json: String, index: usize) -> Result<String> {
    let mut windows: Vec<WeeklyPeakWindow> = serde_json::from_str(&json)?;
    if index < windows.len() {
        windows.remove(index);
    }
    Ok(serde_json::to_string(&windows)?)
}

#[procedure("/ui/_topcoat/runtime/procedures/save-price-plan")]
pub async fn save_price_plan(
    cx: &Cx,
    csrf: String,
    provider_id: String,
    model_id: String,
    currency: String,
    source_url: String,
    effective_at: String,
    use_schedule: bool,
    timezone: String,
    rules_json: String,
    windows_json: String,
) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let result: std::result::Result<String, StoreError> = async {
        let provider_id = provider_id
            .parse::<i64>()
            .map_err(|_| StoreError::Validation("Provider ID 无效".into()))?;
        let rules = serde_json::from_str::<Vec<PriceRule>>(&rules_json)
            .map_err(|_| StoreError::Validation("价格规则格式无效".into()))?;
        let windows = serde_json::from_str::<Vec<WeeklyPeakWindow>>(&windows_json)
            .map_err(|_| StoreError::Validation("峰时窗口格式无效".into()))?;
        let store = &app_context::<AppState>(cx).store;
        let provider = store.get(provider_id).await?;
        let effective_at = if effective_at.is_empty() {
            None
        } else {
            Some(
                NaiveDateTime::parse_from_str(&effective_at, "%Y-%m-%dT%H:%M")
                    .map_err(|_| {
                        StoreError::Validation("生效时间须为有效的 UTC 日期和时间".into())
                    })?
                    .and_utc()
                    .timestamp(),
            )
        };
        store
            .save_price_plan(PricePlanInput {
                provider_id,
                upstream_model_id: model_id.clone(),
                currency,
                source: PriceSource::Manual,
                source_url: (!source_url.trim().is_empty()).then_some(source_url.trim().to_owned()),
                effective_at,
                schedule: use_schedule.then_some(PriceSchedule {
                    timezone,
                    peak_windows: windows,
                }),
                rules,
            })
            .await?;
        Ok(format!("「{}/{}」参考价格已保存", provider.name, model_id))
    }
    .await;
    Ok(result.map_err(|error| error.to_string()))
}

fn rule_field(
    cx: &Cx,
    rules: &Signal<String>,
    error: &Signal<String>,
    index: usize,
    field: &str,
) -> Attributes {
    attributes! { cx => @input=$(|_event: Event| {
        let previous = rules.get();
        let next = raw!("(() => { const rows = JSON.parse(String(${rules}.get())); const value = String(${_event}.target.value.dehydrate()); const field = ${field}.dehydrate(); const row = rows[Number(${index}.toString())]; if (field === 'item' || field === 'unit_price') row[field] = value; else row.conditions[field] = value === '' ? null : (field === 'time_band' ? value : Number(value)); return cx.hydrate(JSON.stringify(rows)); })()", previous);
        rules.set(next);
        error.set("".to_owned());
    }) }
}

fn window_field(
    cx: &Cx,
    windows: &Signal<String>,
    error: &Signal<String>,
    index: usize,
    field: &str,
) -> Attributes {
    attributes! { cx => @input=$(|_event: Event| {
        let previous = windows.get();
        let next = raw!("(() => { const rows = JSON.parse(String(${windows}.get())); const field = ${field}.dehydrate(); const value = String(${_event}.target.value.dehydrate()); rows[Number(${index}.toString())][field] = field === 'weekday' ? Number(value) : value; return cx.hydrate(JSON.stringify(rows)); })()", previous);
        windows.set(next);
        error.set("".to_owned());
    }) }
}

#[shard("/ui/_topcoat/runtime/shards/price-rule-rows")]
pub async fn price_rule_rows(
    cx: &Cx,
    revision: f64,
    rules: Signal<String>,
    error: Signal<String>,
    rerender: Signal<f64>,
) -> Result<impl View> {
    let _ = revision;
    let rows: Vec<PriceRule> = serde_json::from_str(&rules.get_untracked())?;
    let message = error.get_untracked();
    Ok(view! {
        <div class="overflow-x-auto rounded-md border border-border">
            <div class="grid min-w-[760px] grid-cols-[145px_108px_100px_105px_105px_minmax(120px,1fr)_34px] items-center gap-2 border-b border-border bg-surface px-3 py-2 text-xs font-medium text-secondary">
                <span>"计费项"</span><span>"峰谷"</span><span>"缓存 TTL / 秒"</span><span>"提示词 ≥"</span><span>"提示词 ≤"</span><span>"单价 / M token"</span><span></span>
            </div>
            #[key(index)] for (index, row) in rows.iter().enumerate() {
                <div class="grid min-w-[760px] grid-cols-[145px_108px_100px_105px_105px_minmax(120px,1fr)_34px] items-center gap-2 border-b border-border px-3 py-2 last:border-b-0">
                    <select class="h-9 min-w-0 text-xs" aria-label=(format!("第 {} 条计费项", index + 1)) (rule_field(cx, &rules, &error, index, "item"))>
                        <option value="input" selected=(row.item == PriceItem::Input)>"输入"</option>
                        <option value="input_cache_write" selected=(row.item == PriceItem::InputCacheWrite)>"缓存写入"</option>
                        <option value="input_cache_read" selected=(row.item == PriceItem::InputCacheRead)>"缓存读取"</option>
                        <option value="output" selected=(row.item == PriceItem::Output)>"输出"</option>
                    </select>
                    <select class="h-9 min-w-0 text-xs" aria-label=(format!("第 {} 条峰谷条件", index + 1)) (rule_field(cx, &rules, &error, index, "time_band"))>
                        <option value="" selected=(row.conditions.time_band.is_none())>"不限"</option>
                        <option value="peak" selected=(row.conditions.time_band == Some(TimeBand::Peak))>"峰时"</option>
                        <option value="off_peak" selected=(row.conditions.time_band == Some(TimeBand::OffPeak))>"谷时"</option>
                    </select>
                    <input class="h-9 min-w-0 w-full text-xs" type="number" min="1" aria-label=(format!("第 {} 条缓存有效期，秒", index + 1)) value=(row.conditions.cache_ttl_seconds.map_or(String::new(), |value| value.to_string())) placeholder="—" (rule_field(cx, &rules, &error, index, "cache_ttl_seconds"))>
                    <input class="h-9 min-w-0 w-full text-xs" type="number" min="0" aria-label=(format!("第 {} 条提示词最小 token", index + 1)) value=(row.conditions.prompt_tokens_min.map_or(String::new(), |value| value.to_string())) placeholder="—" (rule_field(cx, &rules, &error, index, "prompt_tokens_min"))>
                    <input class="h-9 min-w-0 w-full text-xs" type="number" min="0" aria-label=(format!("第 {} 条提示词最大 token", index + 1)) value=(row.conditions.prompt_tokens_max.map_or(String::new(), |value| value.to_string())) placeholder="—" (rule_field(cx, &rules, &error, index, "prompt_tokens_max"))>
                    <input class="h-9 min-w-0 w-full text-xs tabular-nums" type="text" inputmode="decimal" aria-label=(format!("第 {} 条单价", index + 1)) value=(row.unit_price.as_str()) placeholder="例如 0.15" (rule_field(cx, &rules, &error, index, "unit_price"))>
                    <button class="flex size-8 items-center justify-center rounded border-0 bg-transparent text-secondary hover:bg-[#fff2f0] hover:text-[#cf1322]" type="button" aria-label=(format!("移除第 {} 条规则", index + 1)) @click=$(async |_event: Event| {
                        rules.set(remove_price_rule(rules.get(), index).await);
                        error.set("".to_owned());
                        rerender.increment();
                    })>"×"</button>
                </div>
                if message.starts_with(&format!("第 {} 条", index + 1)) {
                    <p class="m-0 border-b border-border px-3 pb-2 text-xs text-[#cf1322]" role="alert" :hidden=$(error.get().is_empty())>$(error.get())</p>
                }
            }
        </div>
    })
}

#[shard("/ui/_topcoat/runtime/shards/price-peak-windows")]
pub async fn price_peak_windows(
    cx: &Cx,
    revision: f64,
    windows: Signal<String>,
    error: Signal<String>,
    rerender: Signal<f64>,
) -> Result<impl View> {
    let _ = revision;
    let rows: Vec<WeeklyPeakWindow> = serde_json::from_str(&windows.get_untracked())?;
    Ok(view! {
        <div class="space-y-2">
            #[key(index)] for (index, row) in rows.iter().enumerate() {
                <div class="grid grid-cols-[minmax(130px,1fr)_minmax(110px,1fr)_minmax(110px,1fr)_32px] items-center gap-2 max-[560px]:grid-cols-[1fr_1fr_32px]">
                    <select class="h-9 min-w-0 text-sm max-[560px]:col-span-2" aria-label=(format!("第 {} 个窗口的星期", index + 1)) (window_field(cx, &windows, &error, index, "weekday"))>
                        for (day, label) in [(1, "周一"), (2, "周二"), (3, "周三"), (4, "周四"), (5, "周五"), (6, "周六"), (7, "周日")] {
                            <option value=(day.to_string()) selected=(row.weekday == day)>(label)</option>
                        }
                    </select>
                    <input class="h-9 min-w-0 w-full text-sm" type="time" aria-label=(format!("第 {} 个窗口的开始时间", index + 1)) value=(row.start.as_str()) (window_field(cx, &windows, &error, index, "start"))>
                    <input class="h-9 min-w-0 w-full text-sm" type="time" aria-label=(format!("第 {} 个窗口的结束时间", index + 1)) value=(row.end.as_str()) (window_field(cx, &windows, &error, index, "end"))>
                    <button class="flex size-8 items-center justify-center rounded border-0 bg-transparent text-secondary hover:bg-[#fff2f0] hover:text-[#cf1322]" type="button" aria-label=(format!("移除第 {} 个峰时窗口", index + 1)) @click=$(async |_event: Event| {
                        windows.set(remove_price_window(windows.get(), index).await);
                        error.set("".to_owned());
                        rerender.increment();
                    })>"×"</button>
                </div>
            }
        </div>
    })
}

#[topcoat::view::component]
pub(super) async fn price_editor(
    cx: &Cx,
    editor: &PriceEditor,
    csrf: &str,
    success: &Signal<String>,
    refresh: &Signal<f64>,
) -> Result<impl View> {
    let PriceEditor {
        open,
        busy,
        error,
        provider_id,
        provider_name,
        model_id,
        alias_count,
        version,
        source,
        recorded_at,
        currency,
        source_url,
        effective_at,
        use_schedule,
        timezone,
        rules_json,
        windows_json,
        rules_revision,
        windows_revision,
    } = editor;
    let close = native_dialog_close_attributes(cx, "price-dialog");
    Ok(view! {
        native_dialog(config: NativeDialogConfig::new("price-dialog", "参考价格"),
            open: Some(open), busy: busy, language: UiLanguage::ChineseSimplified,
            attrs: attributes! { class="w-[min(1040px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:px-6 [&_.gr-native-dialog-header]:py-4" },
            <form class="m-0 flex min-h-0 max-h-[calc(100dvh_-_48px)] flex-col" @submit=$(async |event: Event| {
                event.prevent_default();
                if busy.get() { return; }
                busy.set(true);
                error.set("".to_owned());
                let result = save_price_plan(
                    csrf.to_owned(), provider_id.get(), model_id.get(), currency.get(),
                    source_url.get(), effective_at.get(), use_schedule.get(), timezone.get(),
                    rules_json.get(), windows_json.get(),
                ).await;
                busy.set(false);
                if result.is_ok() { open.set(false); success.set(result.unwrap()); refresh.increment(); }
                else { error.set(result.unwrap_err()); rules_revision.increment(); windows_revision.increment(); }
            })>
                <div class="min-h-[300px] overflow-y-auto p-6">
                    <div class="mb-4 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]" role="alert" :hidden=$(if error.get().is_empty() { true } else { error.get().starts_with("第 ") })>$(error.get())</div>
                    <div class="flex flex-wrap items-start justify-between gap-3">
                        <div><h3 class="m-0 text-base font-semibold text-heading">$(provider_name.get())" / "$(model_id.get())</h3><p class="mt-1 mb-0 text-xs text-secondary">$(alias_count.get())" 个客户端别名共用此价格。仅作为参考价格展示。"</p></div>
                        <span class="rounded border border-border bg-surface px-2.5 py-1 text-xs text-secondary">$(if version.get().is_empty() { "新价格" } else { "当前版本" })" "$(version.get())" · "$(source.get())" "$(recorded_at.get())</span>
                    </div>
                    <div class="mt-6 grid grid-cols-[150px_minmax(180px,1fr)_minmax(0,1.5fr)] gap-3 max-[700px]:grid-cols-1">
                        <label class="text-xs font-medium text-heading">"币种"<select class="mt-2 h-9 w-full" :value=$(currency.get()) @change=$(|event: Event| { currency.set(event.target.value); error.set("".to_owned()); })><option value="USD">"美元（USD）"</option><option value="CNY">"人民币（CNY）"</option></select></label>
                        <label class="text-xs font-medium text-heading">"生效时间（UTC，可选）"<input class="mt-2 h-9 w-full" type="datetime-local" :value=$(effective_at.get()) @input=$(|event: Event| { effective_at.set(event.target.value); error.set("".to_owned()); })></label>
                        <label class="text-xs font-medium text-heading">"来源 URL（可选）"<input class="mt-2 h-9 w-full" type="url" placeholder="https://…" :value=$(source_url.get()) @input=$(|event: Event| { source_url.set(event.target.value); error.set("".to_owned()); })></label>
                    </div>
                    <div class="mt-6 flex items-center justify-between gap-3"><div><h4 class="m-0 text-sm font-semibold">"价格规则"</h4><p class="mt-1 mb-0 text-xs text-secondary">"每条单价均为每 100 万 token；缓存读取仍属于输入。空白条件表示不限。"</p></div><button class=(BUTTON) type="button" @click=$(async |_event: Event| { rules_json.set(add_price_rule(rules_json.get()).await); error.set("".to_owned()); rules_revision.increment(); })>"＋ 添加规则"</button></div>
                    <div class="mt-3">price_rule_rows(revision: $(rules_revision.get()), rules: rules_json.clone(), error: error.clone(), rerender: rules_revision.clone())</div>
                    <div class="mt-6 border-t border-border pt-5">
                        <label class="flex items-center gap-2 text-sm font-medium text-heading"><input type="checkbox" :checked=$(use_schedule.get()) @change=$(|event: Event| { use_schedule.set(event.target.checked); error.set("".to_owned()); })>"使用峰谷时段"</label>
                        <p class="mt-1 mb-0 text-xs text-secondary">"只有规则使用峰时或谷时条件时才需要。跨天窗口拆成两天录入。"</p>
                        <div class="mt-4" :hidden=$(!use_schedule.get())>
                            <div class="mb-3 flex flex-wrap items-end justify-between gap-3">
                                <label class="text-xs font-medium text-heading">"峰谷时区"<select class="mt-1 block h-9 w-[220px] max-w-full" :value=$(timezone.get()) @change=$(|event: Event| { timezone.set(event.target.value); error.set("".to_owned()); })><option value="UTC">"UTC"</option><option value="Asia/Shanghai">"中国时区（UTC+8）"</option></select></label>
                                <button class=(BUTTON) type="button" @click=$(async |_event: Event| { windows_json.set(add_price_window(windows_json.get()).await); error.set("".to_owned()); windows_revision.increment(); })>"＋ 添加峰时窗口"</button>
                            </div>
                            price_peak_windows(revision: $(windows_revision.get()), windows: windows_json.clone(), error: error.clone(), rerender: windows_revision.clone())
                        </div>
                    </div>
                </div>
                <footer class="flex justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4"><button class=(BUTTON) type="button" (close) :disabled=$(busy.get())>"取消"</button><button class=(class!(BUTTON, PRIMARY)) type="submit" :disabled=$(if busy.get() { true } else if rules_json.get() == "[]" { true } else { !error.get().is_empty() })>$(if busy.get() { "保存中…" } else { "保存新版本" })</button></footer>
            </form>
        )
    })
}
