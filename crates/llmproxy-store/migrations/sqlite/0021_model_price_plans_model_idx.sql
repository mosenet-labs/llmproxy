CREATE INDEX model_price_plans_model_idx
    ON model_price_plans(provider_id, upstream_model_id);
