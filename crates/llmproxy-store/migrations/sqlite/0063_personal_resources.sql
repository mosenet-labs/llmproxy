ALTER TABLE providers ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
CREATE INDEX providers_owner_idx ON providers(owner_user_id);
-- #[toasty::breakpoint]
ALTER TABLE model_routes ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
CREATE INDEX model_routes_owner_idx ON model_routes(owner_user_id);
-- #[toasty::breakpoint]
ALTER TABLE subscription_nodes ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
CREATE INDEX subscription_nodes_owner_idx ON subscription_nodes(owner_user_id);
-- #[toasty::breakpoint]
CREATE TABLE groups_owned (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    version BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL,
    owner_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT
);
-- #[toasty::breakpoint]
INSERT INTO groups_owned (id, name, enabled, version, updated_at) SELECT id, name, enabled, version, updated_at FROM groups;
-- #[toasty::breakpoint]
DROP TABLE groups;
-- #[toasty::breakpoint]
ALTER TABLE groups_owned RENAME TO groups;
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX groups_owner_name_idx ON groups(COALESCE(owner_user_id, 0), name);
