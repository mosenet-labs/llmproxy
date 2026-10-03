ALTER TABLE model_routes ADD COLUMN provider_protocol TEXT NOT NULL DEFAULT '';
-- #[toasty::breakpoint]
UPDATE model_routes SET provider_protocol = protocol;
