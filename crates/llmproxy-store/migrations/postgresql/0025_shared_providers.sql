ALTER TABLE providers DROP COLUMN group_id;
CREATE INDEX providers_name_idx ON providers(name);
