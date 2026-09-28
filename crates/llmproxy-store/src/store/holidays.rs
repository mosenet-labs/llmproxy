use super::*;
use crate::{HolidayDate, HolidayKind, holiday::validate_year};

impl ProviderStore {
    /// Read one annual calendar, including both vacation days and adjusted workdays.
    pub async fn list_holidays(&self, year: i64) -> StoreResult<Vec<HolidayDate>> {
        let mut connection = self.connection().await?;
        let rows = HolidayDateRow::all()
            .filter(HolidayDateRow::fields().year().eq(year))
            .order_by(HolidayDateRow::fields().date().asc())
            .exec(&mut connection)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(HolidayDate {
                    date: row.date,
                    name: row.name,
                    kind: HolidayKind::parse(&row.kind)?,
                    source_url: row.source_url,
                })
            })
            .collect()
    }

    /// Import is atomic: a changed official notice replaces the entire year.
    pub async fn replace_holidays(&self, year: i64, dates: Vec<HolidayDate>) -> StoreResult<usize> {
        validate_year(year, &dates)?;
        let count = dates.len();
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        for row in HolidayDateRow::all()
            .filter(HolidayDateRow::fields().year().eq(year))
            .exec(&mut tx)
            .await?
        {
            row.delete().exec(&mut tx).await?;
        }
        let imported_at = now()?;
        for item in dates {
            HolidayDateRow::create()
                .date(item.date)
                .year(year)
                .name(item.name)
                .kind(item.kind.as_str())
                .source_url(item.source_url)
                .imported_at(imported_at)
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(count)
    }
}
