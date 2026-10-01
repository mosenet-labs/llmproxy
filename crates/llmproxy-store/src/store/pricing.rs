use std::collections::{BTreeMap, HashSet};

use super::*;
use crate::{
    PriceConditions, PriceItem, PricePlanInput, PricePlanView, PriceRule, PriceSchedule,
    PriceSource,
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
        let plans = ModelPricePlan::all()
            .filter(ModelPricePlan::fields().is_current().eq(true))
            .exec(&mut tx)
            .await?;
        let mut views = Vec::with_capacity(plans.len());
        for plan in plans {
            views.push(price_view(&mut tx, plan).await?);
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
        find(&mut tx, input.provider_id).await?;
        let mapped = ModelMapping::all()
            .filter(ModelMapping::fields().provider_id().eq(input.provider_id))
            .filter(
                ModelMapping::fields()
                    .upstream_model_id()
                    .eq(input.upstream_model_id.as_str()),
            )
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
    let schedule_json = input
        .schedule
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| StoreError::Internal)?;
    let plan = ModelPricePlan::create()
        .provider_id(input.provider_id)
        .upstream_model_id(input.upstream_model_id)
        .currency(input.currency)
        .source_kind(input.source.as_str())
        .source_url(input.source_url)
        .recorded_at(recorded_at)
        .effective_at(input.effective_at)
        .is_current(true)
        .schedule_json(schedule_json)
        .exec(&mut *tx)
        .await?;
    for rule in input.rules {
        let conditions_json =
            serde_json::to_string(&rule.conditions).map_err(|_| StoreError::Internal)?;
        ModelPriceRule::create()
            .price_plan_id(plan.id)
            .item_code(rule.item.as_str())
            .unit_code("token")
            .unit_size(1_000_000_i64)
            .conditions_json(conditions_json)
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
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        if row.unit_code != "token" || row.unit_size != 1_000_000 {
            return Err(StoreError::Internal);
        }
        rules.push(PriceRule {
            item: PriceItem::parse(&row.item_code)?,
            conditions: serde_json::from_str(&row.conditions_json)
                .map_err(|_| StoreError::Internal)?,
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
        schedule: plan
            .schedule_json
            .as_deref()
            .map(serde_json::from_str::<PriceSchedule>)
            .transpose()
            .map_err(|_| StoreError::Internal)?,
        rules,
    })
}

/// Move unambiguous legacy values once. A conflicting or incomplete group is
/// left untouched so the console can ask for a deliberate price decision.
pub(super) async fn backfill_legacy_prices(tx: &mut Transaction<'_>) -> StoreResult<()> {
    let current: HashSet<_> = ModelPricePlan::all()
        .filter(ModelPricePlan::fields().is_current().eq(true))
        .exec(&mut *tx)
        .await?
        .into_iter()
        .map(|plan| (plan.provider_id, plan.upstream_model_id))
        .collect();
    type LegacyPrices = (Option<String>, Option<String>);
    let mut groups: BTreeMap<(i64, String), Vec<LegacyPrices>> = BTreeMap::new();
    for mapping in ModelMapping::all().exec(&mut *tx).await? {
        groups
            .entry((mapping.provider_id, mapping.upstream_model_id))
            .or_default()
            .push((
                mapping.input_price_per_million,
                mapping.output_price_per_million,
            ));
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
