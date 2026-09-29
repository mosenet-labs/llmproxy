INSERT INTO model_route_targets (route_id, model_id, position)
SELECT route.id, model.id, 0
FROM model_mappings AS model
JOIN model_routes AS route ON route.name = model.alias;
