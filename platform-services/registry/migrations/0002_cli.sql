CREATE TABLE IF NOT EXISTS cli_packages (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE CHECK (name ~ '^[a-z][a-z0-9-]{0,38}$'),
    description TEXT NOT NULL DEFAULT '',
    binary_name TEXT NOT NULL,
    aliases     TEXT[] NOT NULL DEFAULT '{}',
    created_by  TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cli_versions (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    package_id        UUID NOT NULL REFERENCES cli_packages (id) ON DELETE CASCADE,
    version           TEXT NOT NULL,
    status            TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'published')),
    targets           TEXT[] NOT NULL,
    description       TEXT NOT NULL DEFAULT '',
    binary_name       TEXT NOT NULL,
    aliases           TEXT[] NOT NULL DEFAULT '{}',
    auth              JSONB,
    published_by      TEXT,
    publisher_kind    TEXT,
    source_repository TEXT,
    source_ref        TEXT,
    source_sha        TEXT,
    source_run_id     TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at      TIMESTAMPTZ,
    yanked_at         TIMESTAMPTZ,
    yanked_by         TEXT,
    yank_reason       TEXT,
    UNIQUE (package_id, version)
);

CREATE INDEX IF NOT EXISTS cli_versions_drafts_idx ON cli_versions (created_at) WHERE status = 'draft';

CREATE TABLE IF NOT EXISTS cli_artifacts (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    version_id  UUID NOT NULL REFERENCES cli_versions (id) ON DELETE CASCADE,
    target      TEXT NOT NULL,
    sha256      TEXT NOT NULL,
    size        BIGINT NOT NULL,
    object_key  TEXT NOT NULL,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (version_id, target)
);

CREATE OR REPLACE FUNCTION cli_artifacts_immutable() RETURNS trigger AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM cli_versions WHERE id = NEW.version_id AND status = 'published') THEN
        RAISE EXCEPTION 'version % is published; artifacts are immutable', NEW.version_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS cli_artifacts_immutable ON cli_artifacts;
CREATE TRIGGER cli_artifacts_immutable
    BEFORE INSERT OR UPDATE ON cli_artifacts
    FOR EACH ROW EXECUTE FUNCTION cli_artifacts_immutable();
