DELETE FROM model_routes AS route
WHERE route.id IN (
    SELECT route.id
    FROM model_routes AS route
    JOIN model_mappings AS model ON model.alias = route.name
    JOIN model_route_targets AS target ON target.route_id = route.id
    GROUP BY route.id, model.id
    HAVING COUNT(target.id) = 1 AND MIN(target.model_id) = model.id
);
