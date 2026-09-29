CREATE TABLE model_route_targets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    route_id INTEGER NOT NULL REFERENCES model_routes(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES model_mappings(id) ON DELETE RESTRICT,
    position INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    UNIQUE (route_id, model_id),
    UNIQUE (route_id, position)
);
