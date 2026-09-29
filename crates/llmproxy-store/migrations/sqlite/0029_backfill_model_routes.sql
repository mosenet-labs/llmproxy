INSERT INTO model_routes (name, updated_at)
SELECT alias, updated_at FROM model_mappings;
