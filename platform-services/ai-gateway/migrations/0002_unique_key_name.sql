DROP INDEX IF EXISTS virtual_keys_name_idx;
CREATE UNIQUE INDEX IF NOT EXISTS virtual_keys_name_key ON virtual_keys (name);
