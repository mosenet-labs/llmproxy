INSERT INTO model_routes (name, protocol, enabled, version, updated_at)
SELECT route.name, protocols.protocol, route.enabled, route.version, route.updated_at
FROM legacy_model_routes AS route
CROSS JOIN (
    SELECT 'openai_chat' AS protocol
    UNION ALL SELECT 'openai_responses'
    UNION ALL SELECT 'anthropic_messages'
) AS protocols
WHERE EXISTS (
    SELECT 1 FROM legacy_model_route_targets AS target
    JOIN model_mappings AS model ON model.id = target.model_id
    WHERE target.route_id = route.id
      AND CASE protocols.protocol
          WHEN 'openai_chat' THEN model.openai_chat
          WHEN 'openai_responses' THEN model.openai_responses
          ELSE model.anthropic_messages
      END
);
