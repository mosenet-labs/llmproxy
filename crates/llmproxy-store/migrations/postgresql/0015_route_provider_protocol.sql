ALTER TABLE model_routes ADD COLUMN provider_protocol TEXT;
-- #[toasty::breakpoint]
UPDATE model_routes SET provider_protocol = protocol;
-- #[toasty::breakpoint]
ALTER TABLE model_routes ALTER COLUMN provider_protocol SET NOT NULL;
