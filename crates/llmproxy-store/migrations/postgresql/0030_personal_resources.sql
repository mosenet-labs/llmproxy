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
ALTER TABLE groups ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
ALTER TABLE groups DROP CONSTRAINT groups_name_key;
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX groups_owner_name_idx ON groups(COALESCE(owner_user_id, 0), name);
