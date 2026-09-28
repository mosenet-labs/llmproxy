use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{HolidayDate, HolidayKind, StoreError, StoreResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceItem {
    Input,
    InputCacheWrite,
    InputCacheRead,
    Output,
}

impl PriceItem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::InputCacheWrite => "input_cache_write",
            Self::InputCacheRead => "input_cache_read",
            Self::Output => "output",
        }
    }

    pub(crate) fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "input" => Ok(Self::Input),
            "input_cache_write" => Ok(Self::InputCacheWrite),
            "input_cache_read" => Ok(Self::InputCacheRead),
            "output" => Ok(Self::Output),
            _ => Err(StoreError::Internal),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    Manual,
    Catalog,
    Legacy,
}

impl PriceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Catalog => "catalog",
            Self::Legacy => "legacy",
        }
    }

    pub(crate) fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "manual" => Ok(Self::Manual),
            "catalog" => Ok(Self::Catalog),
            "legacy" => Ok(Self::Legacy),
            _ => Err(StoreError::Internal),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBand {
    Peak,
    OffPeak,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PriceConditions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_band: Option<TimeBand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_ttl_seconds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_min: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_max: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyPeakWindow {
    /// ISO weekday: Monday=1, Sunday=7.
    pub weekday: u8,
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceSchedule {
    pub timezone: String,
    pub peak_windows: Vec<WeeklyPeakWindow>,
    #[serde(default)]
    pub china_holidays_off_peak: bool,
}

/// Resolve a price schedule at a UTC instant. `None` means the annual Chinese
/// holiday calendar is required but has not been imported.
pub fn resolve_time_band(
    schedule: &PriceSchedule,
    instant: DateTime<Utc>,
    china_holidays: Option<&[HolidayDate]>,
) -> StoreResult<Option<TimeBand>> {
    validate_schedule(schedule)?;
    let china_offset = FixedOffset::east_opt(8 * 3600).expect("valid China UTC offset");
    let local = if schedule.timezone == "Asia/Shanghai" {
        instant.with_timezone(&china_offset).fixed_offset()
    } else {
        instant.fixed_offset()
    };
    let weekday = local.weekday().number_from_monday() as u8;
    let minutes = local.hour() * 60 + local.minute();
    let in_peak_window = schedule.peak_windows.iter().any(|window| {
        window.weekday == weekday
            && clock_minutes(&window.start).is_some_and(|start| minutes >= u32::from(start))
            && clock_minutes(&window.end).is_some_and(|end| minutes < u32::from(end))
    });
    if !in_peak_window {
        return Ok(Some(TimeBand::OffPeak));
    }
    if !schedule.china_holidays_off_peak {
        return Ok(Some(TimeBand::Peak));
    }
    let Some(holidays) = china_holidays.filter(|days| !days.is_empty()) else {
        return Ok(None);
    };
    let china_date = instant
        .with_timezone(&china_offset)
        .date_naive()
        .to_string();
    Ok(Some(
        if holidays
            .iter()
            .any(|day| day.date == china_date && day.kind == HolidayKind::Holiday)
        {
            TimeBand::OffPeak
        } else {
            TimeBand::Peak
        },
    ))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceRule {
    pub item: PriceItem,
    pub conditions: PriceConditions,
    /// Decimal amount in `currency` per 1,000,000 tokens.
    pub unit_price: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricePlanInput {
    pub provider_id: i64,
    pub upstream_model_id: String,
    pub currency: String,
    pub source: PriceSource,
    pub source_url: Option<String>,
    pub effective_at: Option<i64>,
    pub schedule: Option<PriceSchedule>,
    pub rules: Vec<PriceRule>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PricePlanView {
    pub id: i64,
    pub provider_id: i64,
    pub upstream_model_id: String,
    pub currency: String,
    pub source: PriceSource,
    pub source_url: Option<String>,
    pub recorded_at: i64,
    pub effective_at: Option<i64>,
    pub schedule: Option<PriceSchedule>,
    pub rules: Vec<PriceRule>,
}

pub(crate) fn validate_price(mut input: PricePlanInput) -> StoreResult<PricePlanInput> {
    if input.upstream_model_id.is_empty()
        || input.upstream_model_id.len() > 200
        || input.upstream_model_id.chars().any(char::is_control)
    {
        return Err(StoreError::Validation("上游模型 ID 无效".into()));
    }
    input.currency.make_ascii_uppercase();
    if !matches!(input.currency.as_str(), "USD" | "CNY") {
        return Err(StoreError::Validation(
            "币种仅支持人民币（CNY）或美元（USD）".into(),
        ));
    }
    if let Some(url) = &input.source_url
        && (url.len() > 2048 || !(url.starts_with("https://") || url.starts_with("http://")))
    {
        return Err(StoreError::Validation(
            "价格来源 URL 须为 HTTP(S) 地址".into(),
        ));
    }
    if input.rules.is_empty() || input.rules.len() > 100 {
        return Err(StoreError::Validation("请添加 1–100 条价格规则".into()));
    }
    let has_bands = input
        .rules
        .iter()
        .any(|rule| rule.conditions.time_band.is_some());
    match &input.schedule {
        Some(schedule) if has_bands => validate_schedule(schedule)?,
        None if has_bands => {
            return Err(StoreError::Validation("峰谷价格需要填写峰时日程".into()));
        }
        Some(_) => {
            return Err(StoreError::Validation(
                "未使用峰谷价格时无需峰时日程".into(),
            ));
        }
        None => {}
    }
    for index in 0..input.rules.len() {
        let (previous_rules, current) = input.rules.split_at_mut(index);
        let rule = &mut current[0];
        rule.unit_price = decimal_price(&rule.unit_price)
            .map_err(|_| StoreError::Validation(format!("第 {} 条规则的单价无效", index + 1)))?;
        if let Some(ttl) = rule.conditions.cache_ttl_seconds
            && (rule.item != PriceItem::InputCacheWrite || ttl == 0)
        {
            return Err(StoreError::Validation(format!(
                "第 {} 条规则的缓存有效期仅适用于缓存写入且须大于 0",
                index + 1
            )));
        }
        if let (Some(min), Some(max)) = (
            rule.conditions.prompt_tokens_min,
            rule.conditions.prompt_tokens_max,
        ) && min > max
        {
            return Err(StoreError::Validation(format!(
                "第 {} 条规则的提示词 token 范围无效",
                index + 1
            )));
        }
        for previous in previous_rules {
            if previous.item == rule.item
                && conditions_overlap(&previous.conditions, &rule.conditions)
            {
                return Err(StoreError::Validation(format!(
                    "第 {} 条规则与同一计费项的已有条件重叠",
                    index + 1
                )));
            }
        }
    }
    Ok(input)
}

pub(crate) fn decimal_price(value: &str) -> StoreResult<String> {
    let valid = !value.is_empty()
        && value.len() <= 32
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && value.bytes().filter(|byte| *byte == b'.').count() <= 1;
    if !valid {
        return Err(StoreError::Validation("价格须为非负十进制数".into()));
    }
    let price = Decimal::from_str_exact(value)
        .map_err(|_| StoreError::Validation("价格须为非负十进制数".into()))?;
    Ok(price.normalize().to_string())
}

fn conditions_overlap(left: &PriceConditions, right: &PriceConditions) -> bool {
    (left.time_band.is_none() || right.time_band.is_none() || left.time_band == right.time_band)
        && (left.cache_ttl_seconds.is_none()
            || right.cache_ttl_seconds.is_none()
            || left.cache_ttl_seconds == right.cache_ttl_seconds)
        && left.prompt_tokens_min.unwrap_or(0) <= right.prompt_tokens_max.unwrap_or(u64::MAX)
        && right.prompt_tokens_min.unwrap_or(0) <= left.prompt_tokens_max.unwrap_or(u64::MAX)
}

fn clock_minutes(value: &str) -> Option<u16> {
    let (hour, minute) = value.split_once(':')?;
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    let hour = hour.parse::<u16>().ok()?;
    let minute = minute.parse::<u16>().ok()?;
    (hour < 24 && minute < 60 || hour == 24 && minute == 0).then_some(hour * 60 + minute)
}

fn validate_schedule(schedule: &PriceSchedule) -> StoreResult<()> {
    if !matches!(schedule.timezone.as_str(), "UTC" | "Asia/Shanghai") {
        return Err(StoreError::Validation(
            "峰谷时区仅支持 UTC 或中国时区（Asia/Shanghai）".into(),
        ));
    }
    if schedule.peak_windows.is_empty() || schedule.peak_windows.len() > 100 {
        return Err(StoreError::Validation("请填写 1–100 个峰时时间窗口".into()));
    }
    let mut windows = Vec::with_capacity(schedule.peak_windows.len());
    for window in &schedule.peak_windows {
        let Some(start) = clock_minutes(&window.start) else {
            return Err(StoreError::Validation("峰时起始时间无效".into()));
        };
        let Some(end) = clock_minutes(&window.end) else {
            return Err(StoreError::Validation("峰时结束时间无效".into()));
        };
        if !(1..=7).contains(&window.weekday) || start >= end || start == 1440 {
            return Err(StoreError::Validation(
                "峰时窗口须在同一天内且起始早于结束；跨天请拆成两个窗口".into(),
            ));
        }
        if windows
            .iter()
            .any(|(day, from, to)| *day == window.weekday && start < *to && *from < end)
        {
            return Err(StoreError::Validation("峰时窗口相互重叠".into()));
        }
        windows.push((window.weekday, start, end));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn plan(rules: Vec<PriceRule>) -> PricePlanInput {
        PricePlanInput {
            provider_id: 1,
            upstream_model_id: "model".into(),
            currency: "usd".into(),
            source: PriceSource::Manual,
            source_url: None,
            effective_at: None,
            schedule: None,
            rules,
        }
    }

    fn rule(item: PriceItem, price: &str) -> PriceRule {
        PriceRule {
            item,
            conditions: PriceConditions::default(),
            unit_price: price.into(),
        }
    }

    #[test]
    fn decimal_is_exact_and_zero_is_a_price() {
        assert_eq!(decimal_price("0").unwrap(), "0");
        assert_eq!(decimal_price("0.00000015").unwrap(), "0.00000015");
        assert_eq!(decimal_price("00.1500").unwrap(), "0.15");
        for invalid in ["", "-1", "+1", "1e2", "NaN", "1.", ".1", "1.2.3"] {
            assert!(decimal_price(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn overlapping_conditions_are_rejected() {
        let mut a = rule(PriceItem::Input, "1");
        a.conditions.prompt_tokens_max = Some(200_000);
        let mut b = rule(PriceItem::Input, "2");
        b.conditions.prompt_tokens_min = Some(200_000);
        assert!(validate_price(plan(vec![a.clone(), b.clone()])).is_err());
        b.conditions.prompt_tokens_min = Some(200_001);
        assert!(validate_price(plan(vec![a, b])).is_ok());
    }

    #[test]
    fn validates_peak_schedule_and_cache_write() {
        let mut a = rule(PriceItem::InputCacheWrite, "2.50");
        a.conditions.cache_ttl_seconds = Some(300);
        let mut b = rule(PriceItem::InputCacheWrite, "4");
        b.conditions.cache_ttl_seconds = Some(3600);
        assert!(validate_price(plan(vec![a, b])).is_ok());
        let mut cache_read = rule(PriceItem::InputCacheRead, "0.20");
        assert!(
            validate_price(plan(vec![rule(PriceItem::Input, "2"), cache_read.clone()])).is_ok()
        );
        cache_read.conditions.cache_ttl_seconds = Some(300);
        assert!(validate_price(plan(vec![cache_read])).is_err());
        let mut peak = rule(PriceItem::Input, "0.30");
        peak.conditions.time_band = Some(TimeBand::Peak);
        let mut priced = plan(vec![peak]);
        assert!(validate_price(priced.clone()).is_err());
        priced.schedule = Some(PriceSchedule {
            timezone: "UTC".into(),
            peak_windows: vec![WeeklyPeakWindow {
                weekday: 1,
                start: "01:00".into(),
                end: "04:00".into(),
            }],
            china_holidays_off_peak: false,
        });
        assert!(validate_price(priced.clone()).is_ok());
        let mut china_timezone = priced.clone();
        china_timezone.currency = "cny".into();
        china_timezone.schedule.as_mut().unwrap().timezone = "Asia/Shanghai".into();
        assert_eq!(validate_price(china_timezone).unwrap().currency, "CNY");
        let mut invalid_currency = priced.clone();
        invalid_currency.currency = "GBP".into();
        assert!(validate_price(invalid_currency).is_err());
        let mut invalid_timezone = priced.clone();
        invalid_timezone.schedule.as_mut().unwrap().timezone = "Europe/London".into();
        assert!(validate_price(invalid_timezone).is_err());
        priced
            .schedule
            .as_mut()
            .unwrap()
            .peak_windows
            .push(WeeklyPeakWindow {
                weekday: 1,
                start: "04:00".into(),
                end: "05:00".into(),
            });
        assert!(validate_price(priced.clone()).is_ok());
        priced.schedule.as_mut().unwrap().peak_windows[1].start = "03:00".into();
        assert!(validate_price(priced).is_err());
    }

    #[test]
    fn chinese_holidays_override_weekday_peak_windows() {
        let schedule = PriceSchedule {
            timezone: "Asia/Shanghai".into(),
            peak_windows: (1..=5)
                .flat_map(|weekday| {
                    [("09:00", "12:00"), ("14:00", "18:00")]
                        .into_iter()
                        .map(move |(start, end)| WeeklyPeakWindow {
                            weekday,
                            start: start.into(),
                            end: end.into(),
                        })
                })
                .collect(),
            china_holidays_off_peak: true,
        };
        let instant = |day, hour| Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap();
        let holidays = [HolidayDate {
            date: "2026-10-02".into(),
            name: "国庆节".into(),
            kind: HolidayKind::Holiday,
            source_url: "https://www.gov.cn/".into(),
        }];
        // 01:00 UTC is 09:00 China time on a Friday in the holiday interval.
        assert_eq!(
            resolve_time_band(&schedule, instant(2, 1), Some(&holidays)).unwrap(),
            Some(TimeBand::OffPeak)
        );
        assert_eq!(
            resolve_time_band(&schedule, instant(2, 1), None).unwrap(),
            None
        );
        // 04:00 UTC is the excluded 12:00 boundary; calendar data is unnecessary.
        assert_eq!(
            resolve_time_band(&schedule, instant(2, 4), None).unwrap(),
            Some(TimeBand::OffPeak)
        );
        assert_eq!(
            resolve_time_band(&schedule, instant(3, 1), None).unwrap(),
            Some(TimeBand::OffPeak)
        );
        let regular = Utc.with_ymd_and_hms(2026, 9, 28, 1, 0, 0).unwrap();
        assert_eq!(
            resolve_time_band(&schedule, regular, Some(&holidays)).unwrap(),
            Some(TimeBand::Peak)
        );
        let adjusted = [HolidayDate {
            date: "2026-09-28".into(),
            name: "调休上班".into(),
            kind: HolidayKind::AdjustedWorkday,
            source_url: "https://www.gov.cn/".into(),
        }];
        assert_eq!(
            resolve_time_band(&schedule, regular, Some(&adjusted)).unwrap(),
            Some(TimeBand::Peak)
        );
    }

    #[test]
    fn old_schedules_default_to_no_holiday_override() {
        let schedule: PriceSchedule = serde_json::from_str(
            r#"{"timezone":"UTC","peak_windows":[{"weekday":1,"start":"01:00","end":"04:00"}]}"#,
        )
        .unwrap();
        assert!(!schedule.china_holidays_off_peak);
    }
}
