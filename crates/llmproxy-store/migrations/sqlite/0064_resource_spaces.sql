CREATE TABLE resource_spaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL CHECK(kind IN ('personal','organization')),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 80),
    personal_user_id BIGINT UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE, created_at BIGINT NOT NULL,
    version BIGINT NOT NULL DEFAULT 0,
    CHECK((kind = 'personal' AND personal_user_id IS NOT NULL) OR (kind = 'organization' AND personal_user_id IS NULL))
);
-- #[toasty::breakpoint]
INSERT INTO resource_spaces(id,kind,name,enabled,created_at) VALUES(1,'organization','默认组织',TRUE,0);
-- #[toasty::breakpoint]
INSERT INTO resource_spaces(id,kind,name,personal_user_id,enabled,created_at) SELECT id+1,'personal','个人空间',id,TRUE,created_at FROM users;
-- #[toasty::breakpoint]
CREATE TABLE organization_members (
    id INTEGER PRIMARY KEY AUTOINCREMENT, space_id BIGINT NOT NULL REFERENCES resource_spaces(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK(role IN ('owner','admin','member')), version BIGINT NOT NULL DEFAULT 0,
    UNIQUE(space_id,user_id)
);
-- #[toasty::breakpoint]
INSERT INTO organization_members(space_id,user_id,role) SELECT 1,id,CASE WHEN id=(SELECT MIN(id) FROM users WHERE role='admin') THEN 'owner' ELSE 'admin' END FROM users WHERE role='admin';
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX organization_single_owner ON organization_members(space_id) WHERE role='owner';
-- #[toasty::breakpoint]
CREATE TABLE organization_invitations (
    id INTEGER PRIMARY KEY AUTOINCREMENT, space_id BIGINT NOT NULL REFERENCES resource_spaces(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK(role IN ('admin','member')), status TEXT NOT NULL CHECK(status IN ('pending','accepted','revoked')),
    expires_at BIGINT NOT NULL, version BIGINT NOT NULL DEFAULT 0
);
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX organization_pending_invite ON organization_invitations(space_id,user_id) WHERE status='pending';
-- #[toasty::breakpoint]
CREATE TABLE group_access (
    id INTEGER PRIMARY KEY AUTOINCREMENT, group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE, UNIQUE(group_id,user_id)
);
-- #[toasty::breakpoint]
ALTER TABLE providers ADD COLUMN space_id BIGINT NOT NULL DEFAULT 1 REFERENCES resource_spaces(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
UPDATE providers SET space_id=CASE WHEN owner_user_id IS NOT NULL THEN owner_user_id+1 ELSE 1 END;
-- #[toasty::breakpoint]
DROP INDEX providers_owner_idx;
-- #[toasty::breakpoint]
ALTER TABLE providers DROP COLUMN owner_user_id;
-- #[toasty::breakpoint]
CREATE INDEX providers_space_idx ON providers(space_id);
-- #[toasty::breakpoint]
ALTER TABLE model_routes ADD COLUMN space_id BIGINT NOT NULL DEFAULT 1 REFERENCES resource_spaces(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
UPDATE model_routes SET space_id=CASE WHEN owner_user_id IS NOT NULL THEN owner_user_id+1 ELSE 1 END;
-- #[toasty::breakpoint]
DROP INDEX model_routes_owner_idx;
-- #[toasty::breakpoint]
ALTER TABLE model_routes DROP COLUMN owner_user_id;
-- #[toasty::breakpoint]
CREATE INDEX model_routes_space_idx ON model_routes(space_id);
-- #[toasty::breakpoint]
ALTER TABLE groups ADD COLUMN space_id BIGINT NOT NULL DEFAULT 1 REFERENCES resource_spaces(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
UPDATE groups SET space_id=CASE WHEN owner_user_id IS NOT NULL THEN owner_user_id+1 ELSE 1 END;
-- #[toasty::breakpoint]
DROP INDEX groups_owner_name_idx;
-- #[toasty::breakpoint]
ALTER TABLE groups DROP COLUMN owner_user_id;
-- #[toasty::breakpoint]
CREATE INDEX groups_space_idx ON groups(space_id);
-- #[toasty::breakpoint]
ALTER TABLE subscription_nodes ADD COLUMN space_id BIGINT  REFERENCES resource_spaces(id) ON DELETE RESTRICT;
-- #[toasty::breakpoint]
UPDATE subscription_nodes SET space_id=CASE WHEN owner_user_id IS NOT NULL THEN owner_user_id+1 WHEN provider_id IS NOT NULL THEN 1 ELSE NULL END;
-- #[toasty::breakpoint]
DROP INDEX subscription_nodes_owner_idx;
-- #[toasty::breakpoint]
ALTER TABLE subscription_nodes DROP COLUMN owner_user_id;
-- #[toasty::breakpoint]
CREATE INDEX subscription_nodes_space_idx ON subscription_nodes(space_id);
-- #[toasty::breakpoint]
CREATE UNIQUE INDEX groups_space_name_idx ON groups(space_id,name);
-- #[toasty::breakpoint]
ALTER TABLE virtual_keys ADD COLUMN created_by_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT;
