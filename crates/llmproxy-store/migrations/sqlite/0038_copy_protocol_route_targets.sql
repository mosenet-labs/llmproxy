INSERT INTO model_route_targets (route_id, model_id, position, enabled)
SELECT new_route.id, target.model_id, target.position, target.enabled
FROM model_routes AS new_route
JOIN legacy_model_routes AS old_route ON old_route.name = new_route.name
JOIN legacy_model_route_targets AS target ON target.route_id = old_route.id
JOIN model_mappings AS model ON model.id = target.model_id
WHERE CASE new_route.protocol
    WHEN 'openai_chat' THEN model.openai_chat
    WHEN 'openai_responses' THEN model.openai_responses
    ELSE model.anthropic_messages
END;
