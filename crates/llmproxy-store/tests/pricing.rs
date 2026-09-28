use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD};
use llmproxy_core::protocol::Protocol;
use llmproxy_store::{
    MessagesAuth, ModelMappingInput, ModelPrice, PriceConditions, PriceItem, PricePlanInput,
    PriceRule, PriceSchedule, PriceSource, ProviderInput, ProviderPaths, ProviderStore, StoreError,
    TimeBand, WeeklyPeakWindow,
};

fn provider() -> ProviderInput {
    ProviderInput {
        name: "deepseek".into(),
        paths: ProviderPaths::single(Protocol::OpenAiChat),
        host: "api.example.com".into(),
        port: 443,
        tls: true,
        api_key: "test-secret".into(),
        enabled: true,
        models_path: "/models".into(),
        models_protocol: Protocol::OpenAiChat,
        anthropic_version: None,
        messages_auth: MessagesAuth::ApiKey,
        connect_timeout_ms: 1_000,
        read_timeout_ms: 1_000,
        write_timeout_ms: 1_000,
    }
}

fn mapping(provider_id: i64, alias: &str, price: Option<ModelPrice>) -> ModelMappingInput {
    ModelMappingInput {
        alias: alias.into(),
        provider_id,
        upstream_model_id: "deepseek-flash".into(),
        protocols: vec![Protocol::OpenAiChat],
        reference_price: price,
    }
}

fn rule(item: PriceItem, band: TimeBand, amount: &str) -> PriceRule {
    PriceRule {
        item,
        conditions: PriceConditions {
            time_band: Some(band),
            ..PriceConditions::default()
        },
        unit_price: amount.into(),
    }
}

async fn exercise(store: &ProviderStore, url: &str) {
    store.migrate().await.unwrap();
    let provider = store.create(provider()).await.unwrap();
    assert!(
        store
            .get_price_plan(provider.id, "deepseek-flash")
            .await
            .unwrap()
            .is_none()
    );
    store
        .create_model(mapping(
            provider.id,
            "deepseek/first",
            Some(ModelPrice {
                input_per_million: "0.15".into(),
                output_per_million: "0.60".into(),
            }),
        ))
        .await
        .unwrap();
    let catalog = store
        .get_price_plan(provider.id, "deepseek-flash")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(catalog.source, PriceSource::Catalog);
    assert_eq!(catalog.rules.len(), 2);
    store
        .create_model(mapping(provider.id, "deepseek/second", None))
        .await
        .unwrap();

    let mut rules = Vec::new();
    for (item, off_peak, peak) in [
        (PriceItem::InputCacheRead, "0.003", "0.006"),
        (PriceItem::Input, "0.15", "0.30"),
        (PriceItem::Output, "0.60", "1.20"),
    ] {
        rules.push(rule(item, TimeBand::OffPeak, off_peak));
        rules.push(rule(item, TimeBand::Peak, peak));
    }
    let plan = PricePlanInput {
        provider_id: provider.id,
        upstream_model_id: "deepseek-flash".into(),
        currency: "usd".into(),
        source: PriceSource::Manual,
        source_url: Some("https://example.com/pricing".into()),
        effective_at: None,
        schedule: Some(PriceSchedule {
            timezone: "UTC".into(),
            peak_windows: vec![WeeklyPeakWindow {
                weekday: 1,
                start: "01:00".into(),
                end: "04:00".into(),
            }],
            china_holidays_off_peak: true,
        }),
        rules,
    };
    let saved = store.save_price_plan(plan.clone()).await.unwrap();
    assert!(saved.id > catalog.id);
    assert_eq!(saved.currency, "USD");
    assert_eq!(saved.rules.len(), 6);
    assert!(saved.schedule.as_ref().unwrap().china_holidays_off_peak);
    assert_eq!(store.list_current_price_plans().await.unwrap().len(), 1);
    store
        .create_model(mapping(
            provider.id,
            "deepseek/third",
            Some(ModelPrice {
                input_per_million: "999".into(),
                output_per_million: "999".into(),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .get_price_plan(provider.id, "deepseek-flash")
            .await
            .unwrap()
            .unwrap()
            .id,
        saved.id,
        "new catalog data must not overwrite a manual version"
    );
    let mut overlap = plan.clone();
    overlap
        .rules
        .push(rule(PriceItem::Input, TimeBand::Peak, "9"));
    assert!(matches!(
        store.save_price_plan(overlap).await,
        Err(StoreError::Validation(_))
    ));
    assert_eq!(
        store
            .get_price_plan(provider.id, "deepseek-flash")
            .await
            .unwrap()
            .unwrap()
            .id,
        saved.id
    );

    let mut china_plan = plan;
    china_plan.currency = "cny".into();
    china_plan.schedule.as_mut().unwrap().timezone = "Asia/Shanghai".into();
    let china_price = store.save_price_plan(china_plan).await.unwrap();
    let stored_china_price = store
        .get_price_plan(provider.id, "deepseek-flash")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_china_price.id, china_price.id);
    assert_eq!(stored_china_price.currency, "CNY");
    assert_eq!(
        stored_china_price.schedule.unwrap().timezone,
        "Asia/Shanghai"
    );

    for (model_id, alias) in [
        ("legacy-conflict", "legacy/conflict-one"),
        ("legacy-conflict", "legacy/conflict-two"),
        ("legacy-consistent", "legacy/consistent-one"),
        ("legacy-consistent", "legacy/consistent-two"),
    ] {
        let mut input = mapping(provider.id, alias, None);
        input.upstream_model_id = model_id.into();
        store.create_model(input).await.unwrap();
    }
    let mut db = toasty::Db::builder().connect(url).await.unwrap();
    for sql in [
        "UPDATE model_mappings SET input_price_per_million='0.1', output_price_per_million='0.2' WHERE alias='legacy/conflict-one'",
        "UPDATE model_mappings SET input_price_per_million='0.3', output_price_per_million='0.4' WHERE alias='legacy/conflict-two'",
        "UPDATE model_mappings SET input_price_per_million='0', output_price_per_million='2' WHERE alias='legacy/consistent-one'",
        "UPDATE model_mappings SET input_price_per_million='0.000', output_price_per_million='2.0' WHERE alias='legacy/consistent-two'",
    ] {
        toasty::sql::statement(sql).exec(&mut db).await.unwrap();
    }
    store.migrate().await.unwrap();
    assert!(
        store
            .get_price_plan(provider.id, "legacy-conflict")
            .await
            .unwrap()
            .is_none()
    );
    let legacy = store
        .get_price_plan(provider.id, "legacy-consistent")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy.source, PriceSource::Legacy);
    assert_eq!(legacy.rules[0].unit_price, "0");
    assert_eq!(legacy.rules[1].unit_price, "2");
    assert_eq!(store.list_current_price_plans().await.unwrap().len(), 2);
}

#[tokio::test]
async fn sqlite_price_versions_and_shared_aliases() {
    let directory = std::env::temp_dir().join(format!(
        "llmproxy-price-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let url = format!("sqlite:{}", directory.join("pricing.sqlite3").display());
    let store = ProviderStore::connect(&url, &STANDARD.encode([5; 32]))
        .await
        .unwrap();
    exercise(&store, &url).await;
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn postgresql_price_versions_and_shared_aliases() {
    let Ok(base_url) = std::env::var("LLMPROXY_TEST_DATABASE_URL") else {
        eprintln!("跳过 PostgreSQL 价格测试：未设置 LLMPROXY_TEST_DATABASE_URL");
        return;
    };
    let mut admin = toasty::Db::builder().connect(&base_url).await.unwrap();
    let schema = format!(
        "llmproxy_price_test_{}_{}",
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
        let store = ProviderStore::connect(&url, &STANDARD.encode([5; 32]))
            .await
            .unwrap();
        exercise(&store, &url).await;
    })
    .await;
    toasty::sql::statement(format!("DROP SCHEMA {schema} CASCADE"))
        .exec(&mut admin)
        .await
        .unwrap();
    result.unwrap();
}
