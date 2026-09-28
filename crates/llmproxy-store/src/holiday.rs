use chrono::{Datelike, NaiveDate};

use crate::{StoreError, StoreResult};

/// Calendar status from an annual State Council holiday notice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HolidayKind {
    Holiday,
    AdjustedWorkday,
}

impl HolidayKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Holiday => "holiday",
            Self::AdjustedWorkday => "adjusted_workday",
        }
    }

    pub(crate) fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "holiday" => Ok(Self::Holiday),
            "adjusted_workday" => Ok(Self::AdjustedWorkday),
            _ => Err(StoreError::Internal),
        }
    }
}

/// A single date from the official notice, including its source URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HolidayDate {
    pub date: String,
    pub name: String,
    pub kind: HolidayKind,
    pub source_url: String,
}

pub(crate) fn validate_year(year: i64, dates: &[HolidayDate]) -> StoreResult<()> {
    if !(2020..=2100).contains(&year) || dates.is_empty() {
        return Err(StoreError::Validation("节假日年份或日期为空".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for item in dates {
        let date = NaiveDate::parse_from_str(&item.date, "%Y-%m-%d")
            .map_err(|_| StoreError::Validation("节假日日期格式无效".into()))?;
        if i64::from(date.year()) != year || !seen.insert(&item.date) {
            return Err(StoreError::Validation("节假日日期重复或年份不符".into()));
        }
        if item.name.trim().is_empty() || !item.source_url.starts_with("https://www.gov.cn/") {
            return Err(StoreError::Validation("节假日名称或官方来源无效".into()));
        }
    }
    Ok(())
}
