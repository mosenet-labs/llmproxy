CREATE UNIQUE INDEX model_price_plans_current_idx
    ON model_price_plans(provider_id, upstream_model_id) WHERE is_current = 1;
