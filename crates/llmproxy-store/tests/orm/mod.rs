use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use llmproxy_core::{
    protocol::Protocol,
    thinking::{Config, Support},
};
use llmproxy_store::{
    ModelMappingInput, ModelRouteInput, ModelRouteTargetInput, PriceConditions, PriceItem,
    PricePlanInput, PriceRule, PriceSource, ProviderStore, StoreError,
};
use tracing::{
    Event, Instrument, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};

type Statements = Arc<Mutex<Vec<String>>>;
#[derive(Clone, Default)]
struct Queries(Arc<Mutex<HashMap<u64, Statements>>>);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Queries {
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, context: Context<'_, S>) {
        if attributes.metadata().name() != "orm_measure" {
            return;
        }
        #[derive(Default)]
        struct Measurement(u64);
        impl Visit for Measurement {
            fn record_u64(&mut self, field: &Field, value: u64) {
                if field.name() == "measurement" {
                    self.0 = value;
                }
            }
            fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
        }
        let mut measurement = Measurement::default();
        attributes.record(&mut measurement);
        let queries = self.0.lock().unwrap();
        context
            .span(id)
            .unwrap()
            .extensions_mut()
            .insert(queries[&measurement.0].clone());
    }
    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        if event.metadata().target() != "toasty::query" {
            return;
        }
        let Some(scope) = context.event_scope(event) else {
            return;
        };
        let Some(queries) = scope
            .from_root()
            .find_map(|span| span.extensions().get::<Statements>().cloned())
        else {
            return;
        };
        struct Statement<'a>(&'a mut Vec<String>);
        impl Visit for Statement<'_> {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                assert_ne!(
                    field.name(),
                    "db.params",
                    "ORM must not log private parameter values"
                );
                if field.name() == "db.statement" {
                    let value = format!("{value:?}");
                    if value.starts_with("SELECT") {
                        self.0.push(value);
                    }
                }
            }
        }
        event.record(&mut Statement(&mut queries.lock().unwrap()));
    }
}

async fn measured<F: Future>(future: F) -> (F::Output, usize) {
    static QUERIES: OnceLock<Queries> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let queries = QUERIES.get_or_init(|| {
        let queries = Queries::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(queries.clone()),
        )
        .unwrap();
        queries
    });
    let measurement = NEXT.fetch_add(1, Ordering::Relaxed);
    let statements = Statements::default();
    queries
        .0
        .lock()
        .unwrap()
        .insert(measurement, statements.clone());
    let result = future
        .instrument(tracing::debug_span!("orm_measure", measurement))
        .await;
    queries.0.lock().unwrap().remove(&measurement);
    let count = statements.lock().unwrap().len();
    (result, count)
}

pub async fn exercise(store: &ProviderStore, url: &str) {
    let first = store
        .create(super::input("ORM A", Protocol::OpenAiChat))
        .await
        .unwrap();
    let second = store
        .create(super::input("ORM B", Protocol::OpenAiChat))
        .await
        .unwrap();
    let mut thinking = Config {
        support: Support::Switchable,
        ..Default::default()
    };
    thinking.enabled.effort = Some("high".into());
    let mapping = |index, provider_id| ModelMappingInput {
        thinking: thinking.clone(),
        alias: format!("orm-model-{index}"),
        provider_id,
        upstream_model_id: format!("upstream-{index}"),
        protocols: vec![Protocol::OpenAiChat],
        reference_price: None,
    };
    let price = |index, provider_id| PricePlanInput {
        provider_id,
        upstream_model_id: format!("upstream-{index}"),
        currency: "USD".into(),
        source: PriceSource::Manual,
        source_url: None,
        effective_at: None,
        schedule: None,
        rules: vec![PriceRule {
            item: PriceItem::Input,
            conditions: PriceConditions::default(),
            unit_price: "0.12345678".into(),
        }],
    };
    let route = |index, id| ModelRouteInput {
        name: format!("orm-route-{index}"),
        protocol: Protocol::OpenAiChat,
        provider_protocol: Protocol::OpenAiChat,
        enabled: true,
        targets: vec![ModelRouteTargetInput {
            model_id: id,
            enabled: true,
        }],
    };
    let initial = store.create_model(mapping(0, first.id)).await.unwrap();
    store.save_price_plan(price(0, first.id)).await.unwrap();
    let initial_route = store.create_route(route(0, initial.id)).await.unwrap();
    let (models, model_queries) = measured(store.list_models()).await;
    let (routes, route_queries) = measured(store.list_routes()).await;
    let (resolved, resolution_queries) = measured(store.load_model_routes()).await;
    let (prices, price_queries) = measured(store.list_current_price_plans()).await;
    assert_eq!(models.unwrap().len(), 1);
    assert_eq!(routes.unwrap().len(), 1);
    assert_eq!(resolved.unwrap().len(), 2);
    assert_eq!(prices.unwrap().len(), 1);
    let mut ids = vec![initial.id];
    for index in 1..10 {
        let provider_id = if index % 2 == 0 { first.id } else { second.id };
        let model = store
            .create_model(mapping(index, provider_id))
            .await
            .unwrap();
        ids.push(model.id);
        store
            .save_price_plan(price(index, provider_id))
            .await
            .unwrap();
        store.create_route(route(index, model.id)).await.unwrap();
    }
    let (models, larger_models) = measured(store.list_models()).await;
    let models = models.unwrap();
    assert_eq!(models.iter().map(|model| model.id).collect::<Vec<_>>(), ids);
    for (index, model) in models.iter().enumerate() {
        assert_eq!(model.thinking, thinking);
        assert_eq!(
            model.provider_name,
            if index % 2 == 0 { "ORM A" } else { "ORM B" }
        );
        assert_eq!(store.get_model(model.id).await.unwrap().thinking, thinking);
    }
    let (routes, larger_routes) = measured(store.list_routes()).await;
    let routes = routes.unwrap();
    assert_eq!(routes.len(), 10);
    for (index, route) in routes.iter().enumerate() {
        assert_eq!(route.targets[0].model.id, ids[index]);
    }
    let (resolved, larger_resolution) = measured(store.load_model_routes()).await;
    assert_eq!(resolved.unwrap().len(), 20);
    let (prices, larger_prices) = measured(store.list_current_price_plans()).await;
    let prices = prices.unwrap();
    assert_eq!(prices.len(), 10);
    assert!(prices.iter().all(|plan| plan.schedule.is_none()
        && plan.rules.len() == 1
        && plan.rules[0].unit_price == "0.12345678"));
    for (label, small, large) in [
        ("models", model_queries, larger_models),
        ("routes", route_queries, larger_routes),
        ("resolution", resolution_queries, larger_resolution),
        ("prices", price_queries, larger_prices),
    ] {
        assert!(
            small > 0 && small <= 8,
            "invalid query measurement: {label}={small}"
        );
        assert_eq!(small, large, "{label}: query count grew with record count");
        eprintln!("ORM reads {label}: 1 record={small}, 10 records={large}");
    }
    // Candidate order and disabled-state fallback must survive batched loading.
    let changed = store
        .update_route(
            initial_route.id,
            initial_route.version,
            ModelRouteInput {
                targets: vec![
                    ModelRouteTargetInput {
                        model_id: ids[1],
                        enabled: false,
                    },
                    ModelRouteTargetInput {
                        model_id: ids[0],
                        enabled: true,
                    },
                ],
                ..route(0, initial.id)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        changed
            .targets
            .iter()
            .map(|target| target.model.id)
            .collect::<Vec<_>>(),
        vec![ids[1], ids[0]]
    );
    assert_eq!(
        store
            .load_model_routes()
            .await
            .unwrap()
            .into_iter()
            .find(|route| route.alias == "orm-route-0")
            .unwrap()
            .upstream_model_id,
        "upstream-0"
    );
    let mut failed = mapping(11, first.id);
    failed.protocols = vec![Protocol::AnthropicMessages];
    assert!(matches!(
        store
            .create_models(vec![mapping(10, first.id), failed])
            .await,
        Err(StoreError::Validation(_))
    ));
    assert_eq!(store.list_models().await.unwrap().len(), 10);
    // Historical TEXT encoding and SQL NULL remain readable after the upgrade.
    let mut db = toasty::Db::builder().connect(url).await.unwrap();
    toasty::sql::statement(format!(
        "UPDATE model_mappings SET thinking_json=NULL WHERE id={}",
        initial.id
    ))
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(
        store.get_model(initial.id).await.unwrap().thinking,
        Config::default()
    );
    let initial_plan = prices
        .iter()
        .find(|plan| plan.upstream_model_id == "upstream-0")
        .unwrap();
    toasty::sql::statement(format!(r#"UPDATE model_price_plans SET schedule_json='{{"timezone":"UTC","peak_windows":[]}}' WHERE id={}"#, initial_plan.id)).exec(&mut db).await.unwrap();
    let legacy = store
        .get_price_plan(first.id, "upstream-0")
        .await
        .unwrap()
        .unwrap();
    assert!(!legacy.schedule.unwrap().china_holidays_off_peak);
    toasty::sql::statement(format!(
        "UPDATE model_mappings SET thinking_json='invalid-json' WHERE id={}",
        initial.id
    ))
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(
        store.get_model(initial.id).await.unwrap_err(),
        StoreError::Internal
    );
    assert_eq!(store.list_models().await.unwrap_err(), StoreError::Internal);
    store.migrate().await.unwrap();
    toasty::sql::statement(format!(
        "UPDATE model_mappings SET thinking_json=NULL WHERE id={}",
        initial.id
    ))
    .exec(&mut db)
    .await
    .unwrap();
    store
        .delete_route(changed.id, changed.version)
        .await
        .unwrap();
    assert_eq!(store.list_routes().await.unwrap().len(), 9);
    assert!(
        store
            .list_models()
            .await
            .unwrap()
            .iter()
            .any(|model| model.id == initial.id)
    );
}
