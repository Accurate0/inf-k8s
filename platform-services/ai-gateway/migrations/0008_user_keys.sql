-- Keys minted on first use for graph users, billed like any other key.
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS kanidm_sub TEXT UNIQUE;
