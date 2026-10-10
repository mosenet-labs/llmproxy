use std::collections::{BTreeMap, HashSet};

use super::*;
use crate::{
    PriceConditions, PriceItem, PricePlanInput, PricePlanView, PriceRule, PriceSource,
    pricing::{decimal_price, validate_price},
};

impl ProviderStore {
    pub async fn get_price_plan(
        &self,
        provider_id: i64,
        upstream_model_id: &str,
    ) -> StoreResult<Option<PricePlanView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        Box::pin(self.find_provider(&mut tx, provider_id)).await?;
        let plan = current_plan(&mut tx, provider_id, upstream_model_id).await?;
        let view = match plan {
            Some(plan) => Some(price_view(&mut tx, plan).await?),
            None => None,
        };
        tx.commit().await?;
        Ok(view)
    }

    pub async fn list_current_price_plans(&self) -> StoreResult<Vec<PricePlanView>> {
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, false).await?;
        self.bindings(&mut tx, false).await?;
        let provider_ids = self.provider_ids(&mut tx).await?;
        let plans = ModelPricePlan::all()
            .filter(ModelPricePlan::fields().provider_id().in_list(provider_ids))
            .filter(ModelPricePlan::fields().is_current().eq(true))
            .exec(&mut tx)
            .await?;
        let mut rules: HashMap<i64, Vec<ModelPriceRule>> = HashMap::new();
        if !plans.is_empty() {
            let rows = ModelPriceRule::all()
                .filter(
                    ModelPriceRule::fields()
                        .price_plan_id()
                        .in_list(plans.iter().map(|plan| plan.id).collect::<Vec<_>>()),
                )
                .order_by(ModelPriceRule::fields().id().asc())
                .exec(&mut tx)
                .await?;
            for row in rows {
                rules.entry(row.price_plan_id).or_default().push(row);
            }
        }
        let mut views = Vec::with_capacity(plans.len());
        for plan in plans {
            let rows = rules.remove(&plan.id).unwrap_or_default();
            views.push(price_view_with_rules(plan, rows)?);
        }
        tx.commit().await?;
        Ok(views)
    }

    /// A new save archives the old current version in the same transaction.
    pub async fn save_price_plan(&self, input: PricePlanInput) -> StoreResult<PricePlanView> {
        let input = validate_price(input)?;
        let mut connection = self.connection().await?;
        let mut tx = self.transaction(&mut connection, true).await?;
        self.bindings(&mut tx, true).await?;
        Box::pin(self.find_provider(&mut tx, input.provider_id)).await?;
        let mapped = ModelMapping::all()
            .filter(ModelMapping::fields().provider_id().eq(input.provider_id))
            .filter(
                ModelMapping::fields()
                    .upstream_model_id()
                    .eq(input.upstream_model_id.as_str()),
            )
            .select(ModelMapping::fields().id())
            .first()
            .exec(&mut tx)
            .await?;
        if mapped.is_none() {
            return Err(StoreError::Validation(
                "请先为此 Provider 导入上游模型".into(),
            ));
        }
        if let Some(mut previous) =
            current_plan(&mut tx, input.provider_id, &input.upstream_model_id).await?
        {
            previous.update().is_current(false).exec(&mut tx).await?;
        }
        let plan = insert_price_plan(&mut tx, input, now()?).await?;
        let view = price_view(&mut tx, plan).await?;
        tx.commit().await?;
        Ok(view)
    }
}

async fn current_plan(
    executor: &mut dyn Executor,
    provider_id: i64,
    upstream_model_id: &str,
) -> StoreResult<Option<ModelPricePlan>> {
    Ok(ModelPricePlan::all()
        .filter(ModelPricePlan::fields().provider_id().eq(provider_id))
        .filter(
            ModelPricePlan::fields()
                .upstream_model_id()
                .eq(upstream_model_id),
        )
        .filter(ModelPricePlan::fields().is_current().eq(true))
        .first()
        .exec(executor)
        .await?)
}

async fn insert_price_plan(
    tx: &mut Transaction<'_>,
    input: PricePlanInput,
    recorded_at: i64,
) -> StoreResult<ModelPricePlan> {
    let plan = ModelPricePlan::create()
        .provider_id(input.provider_id)
        .upstream_model_id(input.upstream_model_id)
        .currency(input.currency)
        .source_kind(input.source.as_str())
        .source_url(input.source_url)
        .recorded_at(recorded_at)
        .effective_at(input.effective_at)
        .is_current(true)
        .schedule_json(input.schedule.map(toasty::Json))
        .exec(&mut *tx)
        .await?;
    for rule in input.rules {
        ModelPriceRule::create()
            .price_plan_id(plan.id)
            .item_code(rule.item.as_str())
            .unit_code("token")
            .unit_size(1_000_000_i64)
            .conditions_json(&rule.conditions)
            .unit_price(rule.unit_price)
            .exec(&mut *tx)
            .await?;
    }
    Ok(plan)
}

async fn price_view(
    executor: &mut dyn Executor,
    plan: ModelPricePlan,
) -> StoreResult<PricePlanView> {
    let rows = ModelPriceRule::all()
        .filter(ModelPriceRule::fields().price_plan_id().eq(plan.id))
        .order_by(ModelPriceRule::fields().id().asc())
        .exec(executor)
        .await?;
    price_view_with_rules(plan, rows)
}

fn price_view_with_rules(
    plan: ModelPricePlan,
    rows: Vec<ModelPriceRule>,
) -> StoreResult<PricePlanView> {
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        if row.unit_code != "token" || row.unit_size != 1_000_000 {
            return Err(StoreError::Internal);
        }
        rules.push(PriceRule {
            item: PriceItem::parse(&row.item_code)?,
            conditions: row.conditions_json.0,
            unit_price: row.unit_price,
        });
    }
    Ok(PricePlanView {
        id: plan.id,
        provider_id: plan.provider_id,
        upstream_model_id: plan.upstream_model_id,
        currency: plan.currency,
        source: PriceSource::parse(&plan.source_kind)?,
        source_url: plan.source_url,
        recorded_at: plan.recorded_at,
        effective_at: plan.effective_at,
        schedule: plan.schedule_json.map(|schedule| schedule.0),
        rules,
    })
}

/// Move unambiguous legacy values once. A conflicting or incomplete group is
/// left untouched so the console can ask for a deliberate price decision.
pub(super) async fn backfill_legacy_prices(tx: &mut Transaction<'_>) -> StoreResult<()> {
    let current: HashSet<_> = ModelPricePlan::all()
        .filter(ModelPricePlan::fields().is_current().eq(true))
        .select((
            ModelPricePlan::fields().provider_id(),
            ModelPricePlan::fields().upstream_model_id(),
        ))
        .exec(&mut *tx)
        .await?
        .into_iter()
        .collect();
    type LegacyPrices = (Option<String>, Option<String>);
    let mut groups: BTreeMap<(i64, String), Vec<LegacyPrices>> = BTreeMap::new();
    for (provider_id, upstream_model_id, input, output) in ModelMapping::all()
        .select((
            ModelMapping::fields().provider_id(),
            ModelMapping::fields().upstream_model_id(),
            ModelMapping::fields().input_price_per_million(),
            ModelMapping::fields().output_price_per_million(),
        ))
        .exec(&mut *tx)
        .await?
    {
        groups
            .entry((provider_id, upstream_model_id))
            .or_default()
            .push((input, output));
    }
    for ((provider_id, upstream_model_id), prices) in groups {
        if current.contains(&(provider_id, upstream_model_id.clone())) {
            continue;
        }
        let Some((Some(input), Some(output))) = prices.first() else {
            continue;
        };
        let (Ok(input), Ok(output)) = (decimal_price(input), decimal_price(output)) else {
            continue;
        };
        if prices.iter().any(|(candidate_input, candidate_output)| {
            candidate_input
                .as_deref()
                .and_then(|value| decimal_price(value).ok())
                .as_deref()
                != Some(input.as_str())
                || candidate_output
                    .as_deref()
                    .and_then(|value| decimal_price(value).ok())
                    .as_deref()
                    != Some(output.as_str())
        }) {
            continue;
        }
        let candidate = PricePlanInput {
            provider_id,
            upstream_model_id,
            currency: "USD".into(),
            source: PriceSource::Legacy,
            source_url: None,
            effective_at: None,
            schedule: None,
            rules: vec![
                PriceRule {
                    item: PriceItem::Input,
                    conditions: PriceConditions::default(),
                    unit_price: input,
                },
                PriceRule {
                    item: PriceItem::Output,
                    conditions: PriceConditions::default(),
                    unit_price: output,
                },
            ],
        };
        // Invalid historical text remains available in the old columns.
        if let Ok(candidate) = validate_price(candidate) {
            insert_price_plan(&mut *tx, candidate, now()?).await?;
        }
    }
    Ok(())
}

pub(super) async fn seed_catalog_price(
    tx: &mut Transaction<'_>,
    provider_id: i64,
    upstream_model_id: &str,
    price: &ModelPrice,
) -> StoreResult<()> {
    if current_plan(&mut *tx, provider_id, upstream_model_id)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let input = validate_price(PricePlanInput {
        provider_id,
        upstream_model_id: upstream_model_id.to_owned(),
        currency: "USD".into(),
        source: PriceSource::Catalog,
        source_url: None,
        effective_at: None,
        schedule: None,
        rules: vec![
            PriceRule {
                item: PriceItem::Input,
                conditions: PriceConditions::default(),
                unit_price: price.input_per_million.clone(),
            },
            PriceRule {
                item: PriceItem::Output,
                conditions: PriceConditions::default(),
                unit_price: price.output_per_million.clone(),
            },
        ],
    })?;
    insert_price_plan(&mut *tx, input, now()?).await?;
    Ok(())
}
