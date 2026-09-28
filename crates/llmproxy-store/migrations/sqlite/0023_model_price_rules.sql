CREATE TABLE model_price_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    price_plan_id INTEGER NOT NULL REFERENCES model_price_plans(id) ON DELETE CASCADE,
    item_code TEXT NOT NULL,
    unit_code TEXT NOT NULL,
    unit_size INTEGER NOT NULL CHECK (unit_size > 0),
    conditions_json TEXT NOT NULL,
    unit_price TEXT NOT NULL
);
