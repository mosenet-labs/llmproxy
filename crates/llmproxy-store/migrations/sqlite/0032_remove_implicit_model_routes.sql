DELETE FROM model_routes
WHERE id IN (
    SELECT route.id
    FROM model_routes AS route
    JOIN model_mappings AS model ON model.alias = route.name
    LEFT JOIN model_route_targets AS target ON target.route_id = route.id
    GROUP BY route.id
    HAVING COUNT(target.id) = 0
);
