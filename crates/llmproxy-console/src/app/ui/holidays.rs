use chrono::{Datelike, Duration, FixedOffset, NaiveDate, Utc};
use llmproxy_store::HolidayKind;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    runtime::{Event, Signal, procedure, shard, signal},
    view::{View, view},
};
use topcoat_ant_design::{
    CalendarEvent, CalendarEventKind, CalendarView, NotificationTone, UiLanguage, calendar,
    notification,
};

use crate::app::{AppState, check_csrf, holiday_notice};

type Outcome = std::result::Result<String, String>;

fn china_today() -> NaiveDate {
    Utc::now()
        .with_timezone(&FixedOffset::east_opt(8 * 3600).expect("China UTC offset"))
        .date_naive()
}

fn selected_mode(value: &str) -> CalendarView {
    match value {
        "day" => CalendarView::Day,
        "week" => CalendarView::Week,
        _ => CalendarView::Month,
    }
}

#[page]
pub async fn holidays(cx: &Cx) -> Result<impl View> {
    let focus = signal(cx, || china_today().to_string());
    let mode = signal(cx, || "month".to_owned());
    let revision = signal(cx, || 0.0);
    let success = signal(cx, String::new);
    let failure = signal(cx, String::new);
    let busy = signal(cx, || false);
    let csrf = crate::app::auth::csrf_token(cx);
    Ok(view! {
        notification(
            message: &success,
            title: "导入成功",
            tone: NotificationTone::Success,
            language: UiLanguage::ChineseSimplified
        )
        notification(
            message: &failure,
            title: "导入失败",
            tone: NotificationTone::Error,
            language: UiLanguage::ChineseSimplified
        )
        <section class=(super::providers::PAGE_HEADING)>
            <div>
                <h1 class="sr-only">"节假日"</h1>
                <p>
                    "按国务院办公厅通知展示完整放假区间与调休上班日。"
                </p>
            </div>
            <button
                class="inline-flex h-9 items-center rounded-md border border-primary bg-primary px-4 text-sm font-medium text-white shadow-sm hover:bg-primary-hover disabled:opacity-50"
                type="button"
                :disabled=$(busy.get())
                @click=$(async |_event: Event| {
                    busy.set(true);
                    let result = import_holidays(csrf.clone()).await;
                    if result.is_ok() {
                        success.set(result.unwrap());
                        failure.set("".to_owned());
                        revision.increment();
                    } else {
                        failure.set(result.unwrap_err());
                        success.set("".to_owned());
                    }
                    busy.set(false);
                })
            >
                "导入 2026 年官方安排"
            </button>
        </section>
        holiday_workspace(
            revision: $(revision.get()),
            focus: $(focus),
            mode: $(mode),
            refresh: $(revision)
        )
    })
}

#[shard("/ui/_topcoat/runtime/shards/holiday-workspace")]
pub async fn holiday_workspace(
    cx: &Cx,
    revision: f64,
    focus: Signal<String>,
    mode: Signal<String>,
    refresh: Signal<f64>,
) -> Result<impl View> {
    let _ = revision;
    let date = NaiveDate::parse_from_str(&focus.get_untracked(), "%Y-%m-%d")
        .unwrap_or_else(|_| china_today());
    let view_mode = selected_mode(&mode.get_untracked());
    let rows = app_context::<AppState>(cx)
        .store
        .list_holidays(i64::from(date.year()))
        .await?;
    let entries: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            Some(CalendarEvent {
                date: NaiveDate::parse_from_str(&row.date, "%Y-%m-%d").ok()?,
                title: match row.kind {
                    HolidayKind::Holiday => format!("{} · 放假", row.name),
                    HolidayKind::AdjustedWorkday => row.name.clone(),
                },
                kind: match row.kind {
                    HolidayKind::Holiday => CalendarEventKind::Holiday,
                    HolidayKind::AdjustedWorkday => CalendarEventKind::Workday,
                },
            })
        })
        .collect();
    let today = china_today().to_string();
    Ok(view! {
        <section
            class="mb-4 flex flex-wrap items-center justify-between gap-3"
            aria-label="日历操作"
        >
            <div class="flex items-center gap-2">
                <button
                    class="h-9 rounded-md border border-border bg-white px-3 text-sm text-heading hover:border-primary hover:text-primary"
                    type="button"
                    aria-label="上一时段"
                    @click=$(async |_event: Event| {
                        focus.set(
                            shift_holiday_date(focus.get(), mode.get(), -1_i32).await,
                        );
                        refresh.increment();
                    })
                >
                    "‹"
                </button>
                <button
                    class="h-9 rounded-md border border-border bg-white px-3 text-sm text-heading hover:border-primary hover:text-primary"
                    type="button"
                    @click=$(|_event: Event| {
                        focus.set(today.clone());
                        refresh.increment();
                    })
                >
                    "今天"
                </button>
                <button
                    class="h-9 rounded-md border border-border bg-white px-3 text-sm text-heading hover:border-primary hover:text-primary"
                    type="button"
                    aria-label="下一时段"
                    @click=$(async |_event: Event| {
                        focus.set(
                            shift_holiday_date(focus.get(), mode.get(), 1_i32).await,
                        );
                        refresh.increment();
                    })
                >
                    "›"
                </button>
            </div>
            <div
                class="inline-flex rounded-md border border-border bg-white p-1"
                role="group"
                aria-label="日历视图"
            >
                for (value, label) in [("day", "日"), ("week", "周"), ("month", "月")] {
                    <button
                        class=(if selected_mode(&mode.get_untracked())
                            == selected_mode(value) {
                            "rounded border-0! bg-primary-soft px-4 py-1.5 text-sm font-medium text-primary shadow-none!"
                        } else {
                            "rounded border-0! bg-transparent px-4 py-1.5 text-sm text-secondary shadow-none! hover:text-heading"
                        })
                        type="button"
                        aria-pressed=(mode.get_untracked() == value)
                        @click=$(|_event: Event| {
                            mode.set(value.to_owned());
                            refresh.increment();
                        })
                    >
                        (label)
                    </button>
                }
            </div>
        </section>
        calendar(
            date: date,
            today: china_today(),
            events: &entries,
            mode: view_mode,
            language: UiLanguage::ChineseSimplified
        )
        <div
            class="mt-4 flex flex-wrap items-center justify-between gap-3 text-xs text-secondary"
        >
            <span>"红色：放假区间　黄色：调休上班"</span>
            <a
                class="text-primary hover:underline"
                href=(holiday_notice::SOURCE_2026)
                target="_blank"
                rel="noopener noreferrer"
            >
                "查看国务院办公厅 2026 年通知 ↗"
            </a>
        </div>
    })
}

#[procedure("/ui/_topcoat/runtime/procedures/shift-holiday-date")]
pub async fn shift_holiday_date(date: String, mode: String, delta: i32) -> Result<String> {
    Ok(shift_date(&date, &mode, delta))
}

fn shift_date(date: &str, mode: &str, delta: i32) -> String {
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap_or_else(|_| china_today());
    let shifted = match selected_mode(mode) {
        CalendarView::Day => date.checked_add_signed(Duration::days(i64::from(delta))),
        CalendarView::Week => date.checked_add_signed(Duration::days(i64::from(delta) * 7)),
        CalendarView::Month => {
            let month_index = date.year() * 12 + date.month0() as i32 + delta;
            let year = month_index.div_euclid(12);
            let month = month_index.rem_euclid(12) as u32 + 1;
            let first = NaiveDate::from_ymd_opt(year, month, 1);
            first.and_then(|first| {
                let next = if month == 12 {
                    NaiveDate::from_ymd_opt(year + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(year, month + 1, 1)
                }?;
                first.with_day(date.day().min((next - Duration::days(1)).day()))
            })
        }
    };
    shifted.unwrap_or(date).to_string()
}

#[procedure("/ui/_topcoat/runtime/procedures/import-holidays")]
pub async fn import_holidays(cx: &Cx, csrf: String) -> Result<Outcome> {
    check_csrf(cx, &csrf)?;
    let result = async {
        let dates = holiday_notice::fetch_2026().await?;
        let count = app_context::<AppState>(cx)
            .store
            .replace_holidays(2026, dates)
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!("已从国务院办公厅通知导入 {count} 个日期"))
    }
    .await;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_navigation_clamps_shorter_month() {
        assert_eq!(shift_date("2026-01-31", "month", 1), "2026-02-28");
        assert_eq!(shift_date("2026-02-28", "week", 1), "2026-03-07");
    }
}
