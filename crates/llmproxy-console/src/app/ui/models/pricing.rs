use super::*;
use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, TimeZone, Utc};
use llmproxy_store::{
    HolidayKind, PriceConditions, PriceItem, PricePlanInput, PriceRule, PriceSchedule, PriceSource,
    TimeBand, WeeklyPeakWindow, resolve_time_band,
};
use rust_decimal::Decimal;

#[topcoat::runtime::record]
#[derive(Clone)]
pub struct PriceRuleRecord {
    pub item: String,
    pub time_band: String,
    pub cache_ttl_seconds: String,
    pub prompt_tokens_min: String,
    pub prompt_tokens_max: String,
    pub unit_price: String,
}

impl From<PriceRule> for PriceRuleRecord {
    fn from(rule: PriceRule) -> Self {
        Self {
            item: rule.item.as_str().to_owned(),
            time_band: match rule.conditions.time_band {
                Some(TimeBand::Peak) => "peak",
                Some(TimeBand::OffPeak) => "off_peak",
                None => "",
            }
            .to_owned(),
            cache_ttl_seconds: rule
                .conditions
                .cache_ttl_seconds
                .map(|v| v.to_string())
                .unwrap_or_default(),
            prompt_tokens_min: rule
                .conditions
                .prompt_tokens_min
                .map(|v| v.to_string())
                .unwrap_or_default(),
            prompt_tokens_max: rule
                .conditions
                .prompt_tokens_max
                .map(|v| v.to_string())
                .unwrap_or_default(),
            unit_price: rule.unit_price,
        }
    }
}

impl TryFrom<PriceRuleRecord> for PriceRule {
    type Error = StoreError;

    fn try_from(rule: PriceRuleRecord) -> std::result::Result<Self, Self::Error> {
        let invalid = || StoreError::Validation("价格规则格式无效".into());
        Ok(Self {
            item: match rule.item.as_str() {
                "input" => PriceItem::Input,
                "input_cache_write" => PriceItem::InputCacheWrite,
                "input_cache_read" => PriceItem::InputCacheRead,
                "output" => PriceItem::Output,
                _ => return Err(invalid()),
            },
            conditions: PriceConditions {
                time_band: match rule.time_band.as_str() {
                    "" => None,
                    "peak" => Some(TimeBand::Peak),
                    "off_peak" => Some(TimeBand::OffPeak),
                    _ => return Err(invalid()),
                },
                cache_ttl_seconds: (!rule.cache_ttl_seconds.is_empty())
                    .then(|| rule.cache_ttl_seconds.parse())
                    .transpose()
                    .map_err(|_| invalid())?,
                prompt_tokens_min: (!rule.prompt_tokens_min.is_empty())
                    .then(|| rule.prompt_tokens_min.parse())
                    .transpose()
                    .map_err(|_| invalid())?,
                prompt_tokens_max: (!rule.prompt_tokens_max.is_empty())
                    .then(|| rule.prompt_tokens_max.parse())
                    .transpose()
                    .map_err(|_| invalid())?,
            },
            unit_price: rule.unit_price,
        })
    }
}

#[topcoat::runtime::record]
#[derive(Clone)]
pub struct PriceWindowRecord {
    pub weekday: String,
    pub start: String,
    pub end: String,
}

impl From<WeeklyPeakWindow> for PriceWindowRecord {
    fn from(window: WeeklyPeakWindow) -> Self {
        Self {
            weekday: window.weekday.to_string(),
            start: window.start,
            end: window.end,
        }
    }
}

impl TryFrom<PriceWindowRecord> for WeeklyPeakWindow {
    type Error = StoreError;

    fn try_from(window: PriceWindowRecord) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            weekday: window
                .weekday
                .parse()
                .map_err(|_| StoreError::Validation("峰时窗口格式无效".into()))?,
            start: window.start,
            end: window.end,
        })
    }
}

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
    china_holidays_off_peak: Signal<bool>,
    matrix_mode: Signal<bool>,
    rules_data: Signal<Vec<PriceRuleRecord>>,
    windows_data: Signal<Vec<PriceWindowRecord>>,
    preview_at: Signal<String>,
    preview_result: Signal<String>,
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
            china_holidays_off_peak: signal(cx, || false),
            matrix_mode: signal(cx, || false),
            rules_data: signal(cx, Vec::<PriceRuleRecord>::new),
            windows_data: signal(cx, Vec::<PriceWindowRecord>::new),
            preview_at: signal(cx, current_china_time),
            preview_result: signal(cx, String::new),
            rules_revision: signal(cx, || 0.0),
            windows_revision: signal(cx, || 0.0),
        }
    }
}

fn china_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("valid China UTC offset")
}

fn current_china_time() -> String {
    Utc::now()
        .with_timezone(&china_offset())
        .format("%Y-%m-%dT%H:%M")
        .to_string()
}

fn matrix_rule_indices(rules: &[PriceRule]) -> Option<[usize; 6]> {
    if rules.len() != 6 {
        return None;
    }
    let mut indices: [Option<usize>; 6] = [None; 6];
    for (index, rule) in rules.iter().enumerate() {
        if rule.conditions.cache_ttl_seconds.is_some()
            || rule.conditions.prompt_tokens_min.is_some()
            || rule.conditions.prompt_tokens_max.is_some()
        {
            return None;
        }
        let item = match rule.item {
            PriceItem::InputCacheRead => 0,
            PriceItem::Input => 2,
            PriceItem::Output => 4,
            PriceItem::InputCacheWrite => return None,
        };
        let band = match rule.conditions.time_band {
            Some(TimeBand::OffPeak) => 0,
            Some(TimeBand::Peak) => 1,
            None => return None,
        };
        if indices[item + band].replace(index).is_some() {
            return None;
        }
    }
    indices
        .into_iter()
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

fn matrix_rules(prices: [String; 6]) -> Vec<PriceRule> {
    let mut rules = Vec::with_capacity(6);
    for (index, item) in [
        PriceItem::InputCacheRead,
        PriceItem::Input,
        PriceItem::Output,
    ]
    .into_iter()
    .enumerate()
    {
        for (band, time_band) in [TimeBand::OffPeak, TimeBand::Peak].into_iter().enumerate() {
            rules.push(PriceRule {
                item,
                conditions: PriceConditions {
                    time_band: Some(time_band),
                    ..Default::default()
                },
                unit_price: prices[index * 2 + band].clone(),
            });
        }
    }
    rules
}

#[derive(Clone, Copy)]
struct WeekdayWindows {
    weekday: u8,
    morning: Option<usize>,
    afternoon: Option<usize>,
}

fn weekday_label(weekday: u8) -> &'static str {
    match weekday {
        1 => "周一",
        2 => "周二",
        3 => "周三",
        4 => "周四",
        5 => "周五",
        6 => "周六",
        _ => "周日",
    }
}

fn grouped_weekday_windows(windows: &[WeeklyPeakWindow]) -> Option<Vec<WeekdayWindows>> {
    let mut days: [WeekdayWindows; 7] = std::array::from_fn(|index| WeekdayWindows {
        weekday: index as u8 + 1,
        morning: None,
        afternoon: None,
    });
    for (index, window) in windows.iter().enumerate() {
        let day = days.get_mut(usize::from(window.weekday.checked_sub(1)?))?;
        let slot = if window.start.as_str() < "12:00" && window.end.as_str() <= "12:00" {
            &mut day.morning
        } else if window.start.as_str() >= "12:00" {
            &mut day.afternoon
        } else {
            return None;
        };
        if slot.replace(index).is_some() {
            return None;
        }
    }
    Some(
        days.into_iter()
            .filter(|day| day.morning.is_some() || day.afternoon.is_some())
            .collect(),
    )
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
            "设置价格".into()
        };
    };
    price_lines(plan).map_or_else(|| "按条件计价".into(), |lines| lines.join(" · "))
}

fn display_price(value: &str) -> String {
    match value.split_once('.') {
        None => format!("{value}.00"),
        Some((_, fraction)) if fraction.len() == 1 => format!("{value}0"),
        _ => value.to_owned(),
    }
}

pub(super) fn price_lines(plan: &PricePlanView) -> Option<Vec<String>> {
    let symbol = match plan.currency.as_str() {
        "USD" => "$",
        "CNY" => "¥",
        _ => return None,
    };
    let prices = plan
        .rules
        .iter()
        .map(|rule| Some((rule.item, Decimal::from_str_exact(&rule.unit_price).ok()?)))
        .collect::<Option<Vec<_>>>()?;
    let mut lines = Vec::with_capacity(2);
    for (label, input) in [("In", true), ("Out", false)] {
        let mut values = prices
            .iter()
            .filter(|(item, _)| (*item != PriceItem::Output) == input)
            .map(|(_, price)| *price);
        let Some(first) = values.next() else {
            continue;
        };
        let (low, high) = values.fold((first, first), |(low, high), price| {
            (low.min(price), high.max(price))
        });
        let same_price = low == high;
        let low = display_price(&low.normalize().to_string());
        let amount = if same_price {
            format!("{symbol}{low}")
        } else {
            format!(
                "{symbol}{low}–{symbol}{}",
                display_price(&high.normalize().to_string())
            )
        };
        lines.push(format!("{label}: {amount}/M"));
    }
    (!lines.is_empty()).then_some(lines)
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
    let china_holidays_off_peak = plan
        .and_then(|plan| plan.schedule.as_ref())
        .is_some_and(|schedule| schedule.china_holidays_off_peak);
    let rules = plan.map_or_else(
        || {
            let mut output = blank_rule();
            output.item = PriceItem::Output;
            vec![blank_rule(), output]
        },
        |plan| plan.rules.clone(),
    );
    let rules_data: Vec<PriceRuleRecord> = rules.iter().cloned().map(Into::into).collect();
    let matrix_mode = matrix_rule_indices(&rules).is_some();
    let windows = plan
        .and_then(|plan| plan.schedule.as_ref())
        .map_or_else(Vec::new, |schedule| schedule.peak_windows.clone());
    let windows_data: Vec<PriceWindowRecord> = windows.into_iter().map(Into::into).collect();
    let preview_at = current_china_time();
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
        china_holidays_off_peak: selected_holidays,
        matrix_mode: selected_matrix,
        rules_data: selected_rules,
        windows_data: selected_windows,
        preview_at: selected_preview_at,
        preview_result: selected_preview_result,
        rules_revision,
        windows_revision,
    } = editor;
    attributes! {
        cx =>
        aria-haspopup="dialog"
        aria-controls="price-dialog"
        @click=$(|_event: Event| {
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
            selected_holidays.set(china_holidays_off_peak);
            selected_matrix.set(matrix_mode);
            selected_rules.set(rules_data.clone());
            selected_windows.set(windows_data.clone());
            selected_preview_at.set(preview_at.to_owned());
            selected_preview_result.set("".to_owned());
            rules_revision.increment();
            windows_revision.increment();
            error.set("".to_owned());
            busy.set(false);
            open.set(true);
        })
    }
}

#[procedure("/ui/_topcoat/runtime/procedures/add-price-rule")]
pub async fn add_price_rule(mut rules: Vec<PriceRuleRecord>) -> Result<Vec<PriceRuleRecord>> {
    if rules.len() < 100 {
        rules.push(blank_rule().into());
    }
    Ok(rules)
}

#[procedure("/ui/_topcoat/runtime/procedures/remove-price-rule")]
pub async fn remove_price_rule(
    mut rules: Vec<PriceRuleRecord>,
    index: usize,
) -> Result<Vec<PriceRuleRecord>> {
    if index < rules.len() {
        rules.remove(index);
    }
    Ok(rules)
}

#[procedure("/ui/_topcoat/runtime/procedures/add-price-window")]
pub async fn add_price_window(
    mut windows: Vec<PriceWindowRecord>,
) -> Result<Vec<PriceWindowRecord>> {
    if windows.len() < 100 {
        for weekday in 1..=7 {
            for (start, end) in [("09:00", "12:00"), ("14:00", "18:00")] {
                let used = windows.iter().any(|window| {
                    window.weekday == weekday.to_string()
                        && if start == "09:00" {
                            window.start.as_str() < "12:00"
                        } else {
                            window.start.as_str() >= "12:00"
                        }
                });
                if !used {
                    windows.push(PriceWindowRecord {
                        weekday: weekday.to_string(),
                        start: start.into(),
                        end: end.into(),
                    });
                    return Ok(windows);
                }
            }
        }
        windows.push(PriceWindowRecord {
            weekday: "1".into(),
            start: "00:00".into(),
            end: "01:00".into(),
        });
    }
    Ok(windows)
}

#[procedure("/ui/_topcoat/runtime/procedures/remove-price-window")]
pub async fn remove_price_window(
    mut windows: Vec<PriceWindowRecord>,
    index: usize,
) -> Result<Vec<PriceWindowRecord>> {
    if index < windows.len() {
        windows.remove(index);
    }
    Ok(windows)
}

#[procedure("/ui/_topcoat/runtime/procedures/deepseek-peak-windows")]
pub async fn deepseek_peak_windows() -> Result<Vec<PriceWindowRecord>> {
    let windows: Vec<_> = (1..=5)
        .flat_map(|weekday| {
            [("09:00", "12:00"), ("14:00", "18:00")]
                .into_iter()
                .map(move |(start, end)| PriceWindowRecord {
                    weekday: weekday.to_string(),
                    start: start.into(),
                    end: end.into(),
                })
        })
        .collect();
    Ok(windows)
}

#[procedure("/ui/_topcoat/runtime/procedures/preview-price-band")]
pub async fn preview_price_band(
    cx: &Cx,
    timezone: String,
    windows_data: Vec<PriceWindowRecord>,
    china_holidays_off_peak: bool,
    preview_at: String,
) -> Result<Outcome> {
    let result: std::result::Result<String, StoreError> = async {
        let local = NaiveDateTime::parse_from_str(&preview_at, "%Y-%m-%dT%H:%M")
            .map_err(|_| StoreError::Validation("请选择有效的中国日期和时间".into()))?;
        let instant = china_offset()
            .from_local_datetime(&local)
            .single()
            .ok_or_else(|| StoreError::Validation("预览时间无效".into()))?
            .with_timezone(&Utc);
        let year = i64::from(local.date().year());
        let holidays = if china_holidays_off_peak {
            app_context::<AppState>(cx)
                .store
                .list_holidays(year)
                .await?
        } else {
            Vec::new()
        };
        let schedule = PriceSchedule {
            timezone,
            peak_windows: windows_data
                .into_iter()
                .map(TryInto::try_into)
                .collect::<std::result::Result<Vec<WeeklyPeakWindow>, _>>()?,
            china_holidays_off_peak,
        };
        let band = resolve_time_band(
            &schedule,
            instant,
            (!holidays.is_empty()).then_some(holidays.as_slice()),
        )?;
        let holiday_override = if band == Some(TimeBand::OffPeak)
            && holidays
                .iter()
                .any(|day| day.date == local.date().to_string() && day.kind == HolidayKind::Holiday)
        {
            let mut weekly_schedule = schedule.clone();
            weekly_schedule.china_holidays_off_peak = false;
            resolve_time_band(&weekly_schedule, instant, None)? == Some(TimeBand::Peak)
        } else {
            false
        };
        Ok(match band {
            Some(TimeBand::Peak) => "峰时".into(),
            Some(TimeBand::OffPeak) if holiday_override => "谷时（中国放假安排）".into(),
            Some(TimeBand::OffPeak) => "谷时".into(),
            None => format!("无法判断：{year} 年中国放假安排尚未导入"),
        })
    }
    .await;
    Ok(result.map_err(|error| error.to_string()))
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
    china_holidays_off_peak: bool,
    rules_data: Vec<PriceRuleRecord>,
    windows_data: Vec<PriceWindowRecord>,
) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let result: std::result::Result<String, StoreError> = async {
        let provider_id = provider_id
            .parse::<i64>()
            .map_err(|_| StoreError::Validation("Provider ID 无效".into()))?;
        let rules = rules_data
            .into_iter()
            .map(TryInto::try_into)
            .collect::<std::result::Result<Vec<PriceRule>, _>>()?;
        let windows = windows_data
            .into_iter()
            .map(TryInto::try_into)
            .collect::<std::result::Result<Vec<WeeklyPeakWindow>, _>>()?;
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
                    china_holidays_off_peak,
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
    rules: &Signal<Vec<PriceRuleRecord>>,
    error: &Signal<String>,
    index: usize,
    field: &str,
) -> Attributes {
    attributes! {
        cx =>
        @input=$(|_event: Event| {
            let previous = rules.get();
            let next = raw!(
                "(() => { const payload = ${rules}.get().dehydrate(); const rows = payload.v; const value = String(${_event}.target.value.dehydrate()); const field = ${field}.dehydrate(); rows[Number(${index}.toString())].v[field] = value; return cx.hydrate(payload); })()",
                previous,
            );
            rules.set(next);
            error.set("".to_owned());
        })
    }
}

fn window_field(
    cx: &Cx,
    windows: &Signal<Vec<PriceWindowRecord>>,
    error: &Signal<String>,
    preview_result: &Signal<String>,
    index: usize,
    field: &str,
) -> Attributes {
    attributes! {
        cx =>
        @input=$(|_event: Event| {
            let previous = windows.get();
            let next = raw!(
                "(() => { const payload = ${windows}.get().dehydrate(); const rows = payload.v; const field = ${field}.dehydrate(); const value = String(${_event}.target.value.dehydrate()); rows[Number(${index}.toString())].v[field] = value; return cx.hydrate(payload); })()",
                previous,
            );
            windows.set(next);
            error.set("".to_owned());
            preview_result.set("".to_owned());
        })
    }
}

#[shard("/ui/_topcoat/runtime/shards/price-rule-rows")]
pub async fn price_rule_rows(
    cx: &Cx,
    revision: f64,
    rules: Signal<Vec<PriceRuleRecord>>,
    error: Signal<String>,
    rerender: Signal<f64>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let rows: Vec<PriceRule> = rules
        .get_untracked()
        .into_iter()
        .map(TryInto::try_into)
        .collect::<std::result::Result<_, _>>()?;
    let message = error.get_untracked();
    Ok(view! {
        <div class="overflow-x-auto rounded-md border border-border">
            <div
                class="grid min-w-[760px] grid-cols-[145px_108px_100px_105px_105px_minmax(120px,1fr)_34px] items-center gap-2 border-b border-border bg-surface px-3 py-2 text-xs font-medium text-secondary"
            >
                <span>"计费项"</span>
                <span>"峰谷"</span>
                <span>"缓存 TTL / 秒"</span>
                <span>"提示词 ≥"</span>
                <span>"提示词 ≤"</span>
                <span>"单价 / M token"</span>
                <span></span>
            </div>
            #[key(index)]
            for (index, row) in rows.iter().enumerate() {
                <div
                    class="grid min-w-[760px] grid-cols-[145px_108px_100px_105px_105px_minmax(120px,1fr)_34px] items-center gap-2 border-b border-border px-3 py-2 last:border-b-0"
                >
                    <select
                        class="h-9 min-w-0 text-xs"
                        aria-label=(format!("第 {} 条计费项", index + 1))
                        (rule_field(cx, &rules, &error, index, "item"))
                    >
                        <option value="input" selected=(row.item == PriceItem::Input)>
                            "输入"
                        </option>
                        <option
                            value="input_cache_write"
                            selected=(row.item == PriceItem::InputCacheWrite)
                        >
                            "缓存写入"
                        </option>
                        <option
                            value="input_cache_read"
                            selected=(row.item == PriceItem::InputCacheRead)
                        >
                            "缓存读取"
                        </option>
                        <option value="output" selected=(row.item == PriceItem::Output)>
                            "输出"
                        </option>
                    </select>
                    <select
                        class="h-9 min-w-0 text-xs"
                        aria-label=(format!("第 {} 条峰谷条件", index + 1))
                        (rule_field(cx, &rules, &error, index, "time_band"))
                    >
                        <option value="" selected=(row.conditions.time_band.is_none())>
                            "不限"
                        </option>
                        <option
                            value="peak"
                            selected=(row.conditions.time_band == Some(TimeBand::Peak))
                        >
                            "峰时"
                        </option>
                        <option
                            value="off_peak"
                            selected=(row.conditions.time_band
                                == Some(TimeBand::OffPeak))
                        >
                            "谷时"
                        </option>
                    </select>
                    <input
                        class="h-9 min-w-0 w-full text-xs"
                        type="number"
                        min="1"
                        aria-label=(format!("第 {} 条缓存有效期，秒", index + 1))
                        value=(row.conditions.cache_ttl_seconds.map_or(
                            String::new(),
                            |value| value.to_string(),
                        ))
                        placeholder="—"
                        (rule_field(cx, &rules, &error, index, "cache_ttl_seconds"))
                    >
                    <input
                        class="h-9 min-w-0 w-full text-xs"
                        type="number"
                        min="0"
                        aria-label=(format!("第 {} 条提示词最小 token", index + 1))
                        value=(row.conditions.prompt_tokens_min.map_or(
                            String::new(),
                            |value| value.to_string(),
                        ))
                        placeholder="—"
                        (rule_field(cx, &rules, &error, index, "prompt_tokens_min"))
                    >
                    <input
                        class="h-9 min-w-0 w-full text-xs"
                        type="number"
                        min="0"
                        aria-label=(format!("第 {} 条提示词最大 token", index + 1))
                        value=(row.conditions.prompt_tokens_max.map_or(
                            String::new(),
                            |value| value.to_string(),
                        ))
                        placeholder="—"
                        (rule_field(cx, &rules, &error, index, "prompt_tokens_max"))
                    >
                    <input
                        class="h-9 min-w-0 w-full text-xs tabular-nums"
                        type="text"
                        inputmode="decimal"
                        aria-label=(format!("第 {} 条单价", index + 1))
                        value=(row.unit_price.as_str())
                        placeholder="例如 0.15"
                        (rule_field(cx, &rules, &error, index, "unit_price"))
                    >
                    <button
                        class="flex size-8 items-center justify-center rounded border-0 bg-transparent text-secondary hover:bg-[#fff2f0] hover:text-[#cf1322]"
                        type="button"
                        aria-label=(format!("移除第 {} 条规则", index + 1))
                        @click=$(async |_event: Event| {
                            rules.set(remove_price_rule(rules.get(), index).await);
                            error.set("".to_owned());
                            rerender.increment();
                        })
                    >
                        "×"
                    </button>
                </div>
                if message.starts_with(&format!("第 {} 条", index + 1)) {
                    <p
                        class="m-0 border-b border-border px-3 pb-2 text-xs text-[#cf1322]"
                        role="alert"
                        :hidden=$(error.get().is_empty())
                    >
                        $(error.get())
                    </p>
                }
            }
        </div>
    })
}

#[shard("/ui/_topcoat/runtime/shards/price-matrix-rows")]
pub async fn price_matrix_rows(
    cx: &Cx,
    revision: f64,
    rules: Signal<Vec<PriceRuleRecord>>,
    error: Signal<String>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let rows: Vec<PriceRule> = rules
        .get_untracked()
        .into_iter()
        .map(TryInto::try_into)
        .collect::<std::result::Result<_, _>>()?;
    let labels = matrix_rule_indices(&rows).map_or_else(Vec::new, |indices| {
        vec![
            ("输入 · 缓存命中", indices[0], indices[1]),
            ("输入 · 缓存未命中", indices[2], indices[3]),
            ("输出", indices[4], indices[5]),
        ]
    });
    Ok(view! {
        <div class="overflow-x-auto rounded-md border border-border">
            <div
                class="grid min-w-[560px] grid-cols-[minmax(160px,1fr)_minmax(150px,1fr)_minmax(150px,1fr)] gap-3 border-b border-border bg-surface px-4 py-2 text-xs font-medium text-secondary"
            >
                <span>"计费项"</span>
                <span>"谷时价 / 百万 token"</span>
                <span>"峰时价 / 百万 token"</span>
            </div>
            for (label, off_peak, peak) in labels {
                <div
                    class="grid min-w-[560px] grid-cols-[minmax(160px,1fr)_minmax(150px,1fr)_minmax(150px,1fr)] items-center gap-3 border-b border-border px-4 py-2 text-sm last:border-b-0"
                >
                    <span>(label)</span>
                    <input
                        class="h-9 min-w-0 w-full tabular-nums"
                        type="text"
                        inputmode="decimal"
                        aria-label=(format!("{label}谷时价"))
                        placeholder="—"
                        value=(rows[off_peak].unit_price.as_str())
                        (rule_field(cx, &rules, &error, off_peak, "unit_price"))
                    >
                    <input
                        class="h-9 min-w-0 w-full tabular-nums"
                        type="text"
                        inputmode="decimal"
                        aria-label=(format!("{label}峰时价"))
                        placeholder="—"
                        value=(rows[peak].unit_price.as_str())
                        (rule_field(cx, &rules, &error, peak, "unit_price"))
                    >
                </div>
            }
        </div>
    })
}

#[shard("/ui/_topcoat/runtime/shards/price-peak-windows")]
pub async fn price_peak_windows(
    cx: &Cx,
    revision: f64,
    windows: Signal<Vec<PriceWindowRecord>>,
    error: Signal<String>,
    preview_result: Signal<String>,
    rerender: Signal<f64>,
) -> Result<impl View> {
    crate::app::request_connection(cx);
    let _ = revision;
    let rows: Vec<WeeklyPeakWindow> = windows
        .get_untracked()
        .into_iter()
        .map(TryInto::try_into)
        .collect::<std::result::Result<_, _>>()?;
    let grouped = grouped_weekday_windows(&rows);
    Ok(view! {
        if let Some(days) = grouped {
            <div class="overflow-x-auto rounded-md border border-border">
                <div
                    class="grid min-w-[720px] grid-cols-[68px_minmax(270px,1fr)_minmax(270px,1fr)] gap-3 border-b border-border bg-surface px-3 py-2 text-xs font-medium text-secondary"
                >
                    <span>"星期"</span>
                    <span>"上午"</span>
                    <span>"下午"</span>
                </div>
                for day in days {
                    let day_label = weekday_label(day.weekday);
                    <div
                        class="grid min-w-[720px] grid-cols-[68px_minmax(270px,1fr)_minmax(270px,1fr)] items-center gap-3 border-b border-border px-3 py-2 last:border-b-0"
                    >
                        <span class="text-sm font-medium text-heading">
                            (day_label)
                        </span>
                        for (period, slot) in [
                            ("上午", day.morning),
                            ("下午", day.afternoon),
                        ] {
                            <div class="flex min-w-0 items-center gap-2">
                                if let Some(index) = slot {
                                    <input
                                        class="h-9 min-w-0 w-full text-sm"
                                        type="time"
                                        aria-label=(format!("{day_label}{period}开始时间"))
                                        value=(rows[index].start.as_str())
                                        (window_field(
                                            cx,
                                            &windows,
                                            &error,
                                            &preview_result,
                                            index,
                                            "start",
                                        ))
                                    >
                                    <span class="shrink-0 text-xs text-muted">"—"</span>
                                    <input
                                        class="h-9 min-w-0 w-full text-sm"
                                        type="time"
                                        aria-label=(format!("{day_label}{period}结束时间"))
                                        value=(rows[index].end.as_str())
                                        (window_field(
                                            cx,
                                            &windows,
                                            &error,
                                            &preview_result,
                                            index,
                                            "end",
                                        ))
                                    >
                                    <button
                                        class="flex size-7 shrink-0 items-center justify-center rounded border-0 bg-transparent text-secondary hover:bg-[#fff2f0] hover:text-[#cf1322]"
                                        type="button"
                                        aria-label=(format!("移除{day_label}{period}峰时窗口"))
                                        @click=$(async |_event: Event| {
                                            windows.set(remove_price_window(windows.get(), index).await);
                                            error.set("".to_owned());
                                            preview_result.set("".to_owned());
                                            rerender.increment();
                                        })
                                    >
                                        "×"
                                    </button>
                                } else {
                                    <span class="text-xs text-muted">"未设置"</span>
                                }
                            </div>
                        }
                    </div>
                }
            </div>
        } else {
            <div class="space-y-2">
                #[key(index)]
                for (index, row) in rows.iter().enumerate() {
                    <div
                        class="grid grid-cols-[minmax(130px,1fr)_minmax(110px,1fr)_minmax(110px,1fr)_32px] items-center gap-2 max-[560px]:grid-cols-[1fr_1fr_32px]"
                    >
                        <select
                            class="h-9 min-w-0 text-sm max-[560px]:col-span-2"
                            aria-label=(format!("第 {} 个窗口的星期", index + 1))
                            (window_field(
                                cx,
                                &windows,
                                &error,
                                &preview_result,
                                index,
                                "weekday",
                            ))
                        >
                            for (weekday, label) in [
                                (1, "周一"),
                                (2, "周二"),
                                (3, "周三"),
                                (4, "周四"),
                                (5, "周五"),
                                (6, "周六"),
                                (7, "周日"),
                            ] {
                                <option
                                    value=(weekday.to_string())
                                    selected=(row.weekday == weekday)
                                >
                                    (label)
                                </option>
                            }
                        </select>
                        <input
                            class="h-9 min-w-0 w-full text-sm"
                            type="time"
                            aria-label=(format!(
                                "第 {} 个窗口的开始时间",
                                index + 1,
                            ))
                            value=(row.start.as_str())
                            (window_field(
                                cx,
                                &windows,
                                &error,
                                &preview_result,
                                index,
                                "start",
                            ))
                        >
                        <input
                            class="h-9 min-w-0 w-full text-sm"
                            type="time"
                            aria-label=(format!(
                                "第 {} 个窗口的结束时间",
                                index + 1,
                            ))
                            value=(row.end.as_str())
                            (window_field(
                                cx,
                                &windows,
                                &error,
                                &preview_result,
                                index,
                                "end",
                            ))
                        >
                        <button
                            class="flex size-8 items-center justify-center rounded border-0 bg-transparent text-secondary hover:bg-[#fff2f0] hover:text-[#cf1322]"
                            type="button"
                            aria-label=(format!(
                                "移除第 {} 个峰时窗口",
                                index + 1,
                            ))
                            @click=$(async |_event: Event| {
                                windows.set(remove_price_window(windows.get(), index).await);
                                error.set("".to_owned());
                                preview_result.set("".to_owned());
                                rerender.increment();
                            })
                        >
                            "×"
                        </button>
                    </div>
                }
            </div>
        }
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
        china_holidays_off_peak,
        matrix_mode,
        rules_data,
        windows_data,
        preview_at,
        preview_result,
        rules_revision,
        windows_revision,
    } = editor;
    let calendar_year = Utc::now().with_timezone(&china_offset()).year();
    let calendar_imported = !app_context::<AppState>(cx)
        .store
        .list_holidays(i64::from(calendar_year))
        .await?
        .is_empty();
    let calendar_status = if calendar_imported {
        format!("已导入 {calendar_year} 年放假安排")
    } else {
        format!("{calendar_year} 年放假安排尚未导入")
    };
    let mut output = blank_rule();
    output.item = PriceItem::Output;
    let blank_rules_data: Vec<PriceRuleRecord> = vec![blank_rule(), output]
        .into_iter()
        .map(Into::into)
        .collect();
    let blank_matrix_data: Vec<PriceRuleRecord> =
        matrix_rules(std::array::from_fn(|_| String::new()))
            .into_iter()
            .map(Into::into)
            .collect();
    let close = native_dialog_close_attributes(cx, "price-dialog");
    Ok(view! {
        native_dialog(
            config: NativeDialogConfig::new("price-dialog", "参考价格"),
            open: Some(open),
            busy: busy,
            language: UiLanguage::ChineseSimplified,
            attrs: attributes! {
                class="w-[min(1040px,calc(100%_-_32px))]! [&_.gr-native-dialog-header]:px-6 [&_.gr-native-dialog-header]:py-4"
            },
            <form
                class="m-0 flex min-h-0 max-h-[calc(100dvh_-_48px)] flex-col"
                @submit=$(async |event: Event| {
                    event.prevent_default();
                    if busy.get() {
                        return;
                    }
                    busy.set(true);
                    error.set("".to_owned());
                    let result = save_price_plan(
                        csrf.to_owned(),
                        provider_id.get(),
                        model_id.get(),
                        currency.get(),
                        source_url.get(),
                        effective_at.get(),
                        use_schedule.get(),
                        timezone.get(),
                        china_holidays_off_peak.get(),
                        rules_data.get(),
                        windows_data.get(),
                    ).await;
                    busy.set(false);
                    if result.is_ok() {
                        open.set(false);
                        success.set(result.unwrap());
                        refresh.increment();
                    } else {
                        error.set(result.unwrap_err());
                        rules_revision.increment();
                        windows_revision.increment();
                    }
                })
            >
                <div class="min-h-[300px] overflow-y-auto p-6">
                    <div
                        class="mb-4 rounded-md border border-[#ffccc7] bg-[#fff2f0] px-4 py-3 text-sm text-[#cf1322]"
                        role="alert"
                        :hidden=$(if error.get().is_empty() {
                            true
                        } else if matrix_mode.get() {
                            false
                        } else {
                            error.get().starts_with("第 ")
                        })
                    >
                        $(error.get())
                    </div>
                    <div class="flex flex-wrap items-start justify-between gap-3">
                        <div>
                            <h3 class="m-0 text-base font-semibold text-heading">
                                $(provider_name.get())
                                " / "
                                $(model_id.get())
                            </h3>
                            <p class="mt-1 mb-0 text-xs text-secondary">
                                $(alias_count.get())
                                " 个模型标识共用此价格。仅作为参考价格展示。"
                            </p>
                        </div>
                        <span
                            class="rounded border border-border bg-surface px-2.5 py-1 text-xs text-secondary"
                        >
                            $(if version.get().is_empty() {
                                "新价格"
                            } else {
                                "当前版本"
                            })
                            " "
                            $(version.get())
                            " · "
                            $(source.get())
                            " "
                            $(recorded_at.get())
                        </span>
                    </div>
                    <div
                        class="mt-6 grid grid-cols-[150px_minmax(180px,1fr)_minmax(0,1.5fr)] gap-3 max-[700px]:grid-cols-1"
                    >
                        <label class="text-xs font-medium text-heading">
                            "币种"
                            <select
                                class="mt-2 h-9 w-full"
                                :value=$(currency.get())
                                @change=$(|event: Event| {
                                    currency.set(event.target.value);
                                    error.set("".to_owned());
                                })
                            >
                                <option value="USD">"美元（USD）"</option>
                                <option value="CNY">"人民币（CNY）"</option>
                            </select>
                        </label>
                        <label class="text-xs font-medium text-heading">
                            "生效时间（UTC，可选）"
                            <input
                                class="mt-2 h-9 w-full"
                                type="datetime-local"
                                :value=$(effective_at.get())
                                @input=$(|event: Event| {
                                    effective_at.set(event.target.value);
                                    error.set("".to_owned());
                                })
                            >
                        </label>
                        <label class="text-xs font-medium text-heading">
                            "来源 URL（可选）"
                            <input
                                class="mt-2 h-9 w-full"
                                type="url"
                                placeholder="https://…"
                                :value=$(source_url.get())
                                @input=$(|event: Event| {
                                    source_url.set(event.target.value);
                                    error.set("".to_owned());
                                })
                            >
                        </label>
                    </div>
                    <div class="mt-6 flex flex-wrap items-center justify-between gap-3">
                        <div>
                            <h4 class="m-0 text-sm font-semibold">"价格规则"</h4>
                            <p class="mt-1 mb-0 text-xs text-secondary">
                                "单价均为每 100 万 token；缓存命中属于输入。"
                            </p>
                        </div>
                        <button
                            class=(BUTTON)
                            type="button"
                            :hidden=$(matrix_mode.get())
                            @click=$(async |_event: Event| {
                                rules_data.set(add_price_rule(rules_data.get()).await);
                                error.set("".to_owned());
                                rules_revision.increment();
                            })
                        >
                            "＋ 添加规则"
                        </button>
                    </div>
                    <div class="mt-3" :hidden=$(matrix_mode.get())>
                        price_rule_rows(
                            revision: $(rules_revision.get()),
                            rules: rules_data.clone(),
                            error: error.clone(),
                            rerender: rules_revision.clone()
                        )
                    </div>
                    <button
                        class="mt-2 border-0 bg-transparent p-0 text-xs text-primary hover:underline"
                        type="button"
                        :hidden=$(if matrix_mode.get() {
                            true
                        } else {
                            !use_schedule.get()
                        })
                        @click=$(|_event: Event| {
                            rules_data.set(blank_matrix_data.clone());
                            matrix_mode.set(true);
                            rules_revision.increment();
                            error.set("".to_owned());
                        })
                    >
                        "清空现有规则并改用 3×2 峰谷价格表"
                    </button>
                    <div class="mt-3" :hidden=$(!matrix_mode.get())>
                        price_matrix_rows(
                            revision: $(rules_revision.get()),
                            rules: rules_data.clone(),
                            error: error.clone()
                        )
                    </div>
                    <div class="mt-6 border-t border-border pt-5">
                        <div class="flex flex-wrap items-center justify-between gap-3">
                            <label
                                class="flex items-center gap-2 text-sm font-medium text-heading"
                            >
                                <input
                                    type="checkbox"
                                    :checked=$(use_schedule.get())
                                    @change=$(|event: Event| {
                                        use_schedule.set(event.target.checked);
                                        preview_result.set("".to_owned());
                                        error.set("".to_owned());
                                    })
                                >
                                "使用峰谷时段"
                            </label>
                            <button
                                class=(BUTTON)
                                type="button"
                                @click=$(async |_event: Event| {
                                    windows_data.set(deepseek_peak_windows().await);
                                    timezone.set("Asia/Shanghai".to_owned());
                                    china_holidays_off_peak.set(true);
                                    use_schedule.set(true);
                                    if raw!(
                                        "cx.hydrate(JSON.stringify(${rules_data}.get().dehydrate()) === JSON.stringify(${blank_rules_data}.dehydrate()))",
                                        false,
                                    ) {
                                        rules_data.set(blank_matrix_data.clone());
                                        matrix_mode.set(true);
                                        rules_revision.increment();
                                    }
                                    windows_revision.increment();
                                    preview_result.set("".to_owned());
                                    error.set("".to_owned());
                                })
                            >
                                "套用 DeepSeek 时段"
                            </button>
                        </div>
                        <p class="mt-1 mb-0 text-xs text-secondary">
                            "DeepSeek 预设峰时：中国时间周一至周五 09:00–12:00、14:00–18:00。预设不会覆盖已有单价。"
                        </p>
                        <div class="mt-4" :hidden=$(!use_schedule.get())>
                            <label class="text-xs font-medium text-heading">
                                "峰谷时区"
                                <select
                                    class="mt-1 block h-9 w-[220px] max-w-full"
                                    :value=$(timezone.get())
                                    @change=$(|event: Event| {
                                        timezone.set(event.target.value);
                                        preview_result.set("".to_owned());
                                        error.set("".to_owned());
                                    })
                                >
                                    <option value="UTC">"UTC"</option>
                                    <option value="Asia/Shanghai">
                                        "中国时区（UTC+8）"
                                    </option>
                                </select>
                            </label>
                            <label
                                class="mt-4 flex items-center gap-2 text-sm text-heading"
                            >
                                <input
                                    type="checkbox"
                                    :checked=$(china_holidays_off_peak.get())
                                    @change=$(|event: Event| {
                                        china_holidays_off_peak.set(event.target.checked);
                                        preview_result.set("".to_owned());
                                        error.set("".to_owned());
                                    })
                                >
                                "中国放假安排全天按谷时"
                            </label>
                            <p class="mt-1 mb-0 text-xs text-secondary">
                                (calendar_status.as_str())
                                " · "
                                <a
                                    class="text-primary hover:underline"
                                    href="/ui/holidays"
                                >
                                    "查看日历"
                                </a>
                                "。调休上班的周末仍按上述星期窗口判定。"
                            </p>
                            <details
                                class="mt-4 rounded-md border border-border px-3 py-2"
                            >
                                <summary
                                    class="cursor-pointer text-sm font-medium text-heading"
                                >
                                    "编辑每周峰时窗口"
                                </summary>
                                <div class="mt-3 flex justify-end">
                                    <button
                                        class=(BUTTON)
                                        type="button"
                                        @click=$(async |_event: Event| {
                                            windows_data.set(add_price_window(windows_data.get()).await);
                                            preview_result.set("".to_owned());
                                            error.set("".to_owned());
                                            windows_revision.increment();
                                        })
                                    >
                                        "＋ 添加峰时窗口"
                                    </button>
                                </div>
                                <p class="my-2 text-xs text-secondary">
                                    "跨天窗口拆成两天录入。"
                                </p>
                                <div class="max-h-80 overflow-y-auto pr-1">
                                    price_peak_windows(
                                        revision: $(windows_revision.get()),
                                        windows: windows_data.clone(),
                                        error: error.clone(),
                                        preview_result: preview_result.clone(),
                                        rerender: windows_revision.clone()
                                    )
                                </div>
                            </details>
                            <div
                                class="mt-4 flex flex-wrap items-end gap-3 rounded-md bg-surface p-3"
                            >
                                <label class="text-xs font-medium text-heading">
                                    "预览时间（中国时间）"
                                    <input
                                        class="mt-1 block h-9 w-[220px] max-w-full"
                                        type="datetime-local"
                                        :value=$(preview_at.get())
                                        @input=$(|event: Event| {
                                            preview_at.set(event.target.value);
                                            preview_result.set("".to_owned());
                                        })
                                    >
                                </label>
                                <button
                                    class=(BUTTON)
                                    type="button"
                                    @click=$(async |_event: Event| {
                                        preview_result.set("判定中…".to_owned());
                                        let result = preview_price_band(
                                            timezone.get(),
                                            windows_data.get(),
                                            china_holidays_off_peak.get(),
                                            preview_at.get(),
                                        ).await;
                                        if result.is_ok() {
                                            preview_result.set(result.unwrap());
                                        } else {
                                            preview_result.set(result.unwrap_err());
                                        }
                                    })
                                >
                                    "预览时段"
                                </button>
                                <span
                                    class="pb-2 text-sm font-medium text-heading"
                                    role="status"
                                >
                                    $(preview_result.get())
                                </span>
                            </div>
                        </div>
                    </div>
                </div>
                <footer
                    class="flex justify-end gap-3 border-t border-border bg-[#fafafa] px-6 py-4"
                >
                    <button
                        class=(BUTTON)
                        type="button"
                        (close)
                        :disabled=$(busy.get())
                    >
                        "取消"
                    </button>
                    <button
                        class=(class!(BUTTON, PRIMARY))
                        type="submit"
                        :disabled=$(if busy.get() {
                            true
                        } else if rules_data.get().is_empty() {
                            true
                        } else {
                            !error.get().is_empty()
                        })
                    >
                        $(if busy.get() { "保存中…" } else { "保存新版本" })
                    </button>
                </footer>
            </form>
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_records_preserve_conditions_and_reject_invalid_input() {
        let rule = PriceRule {
            item: PriceItem::InputCacheWrite,
            conditions: PriceConditions {
                time_band: Some(TimeBand::OffPeak),
                cache_ttl_seconds: Some(300),
                prompt_tokens_min: Some(100),
                prompt_tokens_max: Some(u64::MAX),
            },
            unit_price: "0.000001".into(),
        };
        let record = PriceRuleRecord::from(rule.clone());
        assert_eq!(PriceRule::try_from(record.clone()).unwrap(), rule);
        let mut invalid = record.clone();
        invalid.item = "unknown".into();
        assert!(PriceRule::try_from(invalid).is_err());
        let mut invalid = record.clone();
        invalid.time_band = "unknown".into();
        assert!(PriceRule::try_from(invalid).is_err());
        let mut invalid = record;
        invalid.prompt_tokens_max = "18446744073709551616".into();
        assert!(PriceRule::try_from(invalid).is_err());
        let window = WeeklyPeakWindow {
            weekday: 7,
            start: "09:00".into(),
            end: "12:00".into(),
        };
        assert_eq!(
            WeeklyPeakWindow::try_from(PriceWindowRecord::from(window.clone())).unwrap(),
            window
        );
        assert!(
            WeeklyPeakWindow::try_from(PriceWindowRecord {
                weekday: "-1".into(),
                start: "09:00".into(),
                end: "12:00".into()
            })
            .is_err()
        );
    }

    #[test]
    fn price_labels_use_currency_symbols_without_rounding() {
        let mut plan = PricePlanView {
            id: 1,
            provider_id: 1,
            upstream_model_id: "model".into(),
            currency: "USD".into(),
            source: PriceSource::Manual,
            source_url: None,
            recorded_at: 0,
            effective_at: None,
            schedule: None,
            rules: vec![
                PriceRule {
                    item: PriceItem::Input,
                    conditions: PriceConditions::default(),
                    unit_price: "0.8".into(),
                },
                PriceRule {
                    item: PriceItem::Output,
                    conditions: PriceConditions::default(),
                    unit_price: "2".into(),
                },
            ],
        };
        assert_eq!(price_label(None, false), "设置价格");
        assert_eq!(
            price_lines(&plan),
            Some(vec!["In: $0.80/M".into(), "Out: $2.00/M".into()])
        );
        plan.currency = "CNY".into();
        plan.rules[0].unit_price = "0.003".into();
        assert_eq!(
            price_lines(&plan),
            Some(vec!["In: ¥0.003/M".into(), "Out: ¥2.00/M".into()])
        );
    }

    #[test]
    fn conditional_prices_show_numeric_ranges() {
        let mut plan = PricePlanView {
            id: 1,
            provider_id: 1,
            upstream_model_id: "model".into(),
            currency: "CNY".into(),
            source: PriceSource::Manual,
            source_url: None,
            recorded_at: 0,
            effective_at: None,
            schedule: Some(PriceSchedule {
                timezone: "Asia/Shanghai".into(),
                peak_windows: vec![WeeklyPeakWindow {
                    weekday: 1,
                    start: "09:00".into(),
                    end: "12:00".into(),
                }],
                china_holidays_off_peak: true,
            }),
            rules: vec![
                PriceRule {
                    item: PriceItem::Input,
                    conditions: PriceConditions {
                        time_band: Some(TimeBand::Peak),
                        ..Default::default()
                    },
                    unit_price: "10".into(),
                },
                PriceRule {
                    item: PriceItem::Input,
                    conditions: PriceConditions {
                        time_band: Some(TimeBand::OffPeak),
                        ..Default::default()
                    },
                    unit_price: "2".into(),
                },
                PriceRule {
                    item: PriceItem::InputCacheRead,
                    conditions: PriceConditions::default(),
                    unit_price: "0.5".into(),
                },
                PriceRule {
                    item: PriceItem::Output,
                    conditions: PriceConditions::default(),
                    unit_price: "3".into(),
                },
            ],
        };
        assert_eq!(
            price_lines(&plan),
            Some(vec!["In: ¥0.50–¥10.00/M".into(), "Out: ¥3.00/M".into()])
        );
        assert_eq!(
            price_label(Some(&plan), false),
            "In: ¥0.50–¥10.00/M · Out: ¥3.00/M"
        );
        plan.rules.retain(|rule| rule.item == PriceItem::Output);
        assert_eq!(price_lines(&plan), Some(vec!["Out: ¥3.00/M".into()]));
    }

    #[test]
    fn matrix_fields_follow_the_rule_conditions() {
        let mut rules = matrix_rules([
            "0.003".into(),
            "0.006".into(),
            "0.15".into(),
            "0.30".into(),
            "0.60".into(),
            "1.20".into(),
        ]);
        rules.swap(0, 5);
        assert_eq!(matrix_rule_indices(&rules), Some([5, 1, 2, 3, 4, 0]));
        rules[0].conditions.prompt_tokens_min = Some(1);
        assert_eq!(matrix_rule_indices(&rules), None);
    }

    #[test]
    fn weekly_windows_group_morning_and_afternoon_by_day() {
        let mut windows: Vec<_> = (1..=5)
            .flat_map(|weekday| {
                [("09:00", "12:00"), ("14:00", "18:00")]
                    .into_iter()
                    .map(move |(start, end)| WeeklyPeakWindow {
                        weekday,
                        start: start.into(),
                        end: end.into(),
                    })
            })
            .collect();
        let grouped = grouped_weekday_windows(&windows).unwrap();
        assert_eq!(grouped.len(), 5);
        assert_eq!(
            (grouped[0].morning, grouped[0].afternoon),
            (Some(0), Some(1))
        );
        assert_eq!(
            (grouped[4].morning, grouped[4].afternoon),
            (Some(8), Some(9))
        );

        windows.remove(1);
        let grouped = grouped_weekday_windows(&windows).unwrap();
        assert_eq!((grouped[0].morning, grouped[0].afternoon), (Some(0), None));
        windows.push(WeeklyPeakWindow {
            weekday: 1,
            start: "10:00".into(),
            end: "11:00".into(),
        });
        assert!(grouped_weekday_windows(&windows).is_none());
    }
}
