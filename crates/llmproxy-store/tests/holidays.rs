use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_store::{HolidayDate, HolidayKind, ProviderStore};

const SOURCE: &str = "https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm";

fn date(value: &str, kind: HolidayKind) -> HolidayDate {
    HolidayDate {
        date: value.into(),
        name: "春节".into(),
        kind,
        source_url: SOURCE.into(),
    }
}

#[tokio::test]
async fn sqlite_import_replaces_a_year_atomically() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-holidays-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("holidays.sqlite3").display());
    let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    store
        .replace_holidays(
            2026,
            vec![
                date("2026-02-15", HolidayKind::Holiday),
                date("2026-02-14", HolidayKind::AdjustedWorkday),
            ],
        )
        .await
        .unwrap();
    assert_eq!(store.list_holidays(2026).await.unwrap().len(), 2);
    assert!(
        store
            .replace_holidays(2026, vec![date("2026-02-30", HolidayKind::Holiday)])
            .await
            .is_err()
    );
    assert_eq!(store.list_holidays(2026).await.unwrap().len(), 2);
    store
        .replace_holidays(2026, vec![date("2026-02-15", HolidayKind::Holiday)])
        .await
        .unwrap();
    assert_eq!(store.list_holidays(2026).await.unwrap().len(), 1);
    drop(store);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn postgresql_import_replaces_a_year() {
    let Ok(base_url) = std::env::var("LLMPROXY_TEST_DATABASE_URL") else {
        return;
    };
    let mut admin = toasty::Db::builder().connect(&base_url).await.unwrap();
    let schema = format!(
        "llmproxy_holiday_test_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    toasty::sql::statement(format!("CREATE SCHEMA {schema}"))
        .exec(&mut admin)
        .await
        .unwrap();
    let separator = if base_url.contains('?') { '&' } else { '?' };
    let url = format!("{base_url}{separator}options=-c%20search_path%3D{schema}");
    let result = tokio::spawn(async move {
        let store = ProviderStore::connect(&url, &STANDARD.encode([7; 32]))
            .await
            .unwrap();
        store.migrate().await.unwrap();
        store
            .replace_holidays(
                2026,
                vec![
                    date("2026-02-15", HolidayKind::Holiday),
                    date("2026-02-14", HolidayKind::AdjustedWorkday),
                ],
            )
            .await
            .unwrap();
        assert_eq!(store.list_holidays(2026).await.unwrap().len(), 2);
        store
            .replace_holidays(2026, vec![date("2026-02-15", HolidayKind::Holiday)])
            .await
            .unwrap();
        assert_eq!(store.list_holidays(2026).await.unwrap().len(), 1);
    })
    .await;
    toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
        .exec(&mut admin)
        .await
        .unwrap();
    result.unwrap();
}
