ALTER TABLE cli_versions ADD COLUMN IF NOT EXISTS index_digest TEXT;
ALTER TABLE cli_versions ADD COLUMN IF NOT EXISTS signatures_digest TEXT;

CREATE TABLE IF NOT EXISTS core_catalog (
    id          BOOLEAN PRIMARY KEY DEFAULT true CHECK (id),
    seq         BIGINT NOT NULL DEFAULT 0,
    digest      TEXT,
    dirty       BOOLEAN NOT NULL DEFAULT true,
    rendered_at TIMESTAMPTZ
);

INSERT INTO core_catalog (id) VALUES (true) ON CONFLICT DO NOTHING;
