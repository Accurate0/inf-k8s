CREATE TABLE IF NOT EXISTS core_publish_tokens (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS core_publish_token_scopes (
    token_id  UUID NOT NULL REFERENCES core_publish_tokens (id) ON DELETE CASCADE,
    ecosystem TEXT NOT NULL,
    package   TEXT NOT NULL,
    PRIMARY KEY (token_id, ecosystem, package)
);

CREATE TABLE IF NOT EXISTS core_trusted_publishers (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ecosystem     TEXT NOT NULL,
    package       TEXT NOT NULL,
    provider      TEXT NOT NULL DEFAULT 'forgejo' CHECK (provider = 'forgejo'),
    issuer        TEXT NOT NULL,
    repository    TEXT NOT NULL,
    repository_id TEXT,
    ref_pattern   TEXT,
    workflow      TEXT,
    created_by    TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS core_trusted_publishers_unique_idx ON core_trusted_publishers (
    ecosystem, package, issuer, lower(repository), coalesce(ref_pattern, ''), coalesce(workflow, '')
);

CREATE TABLE IF NOT EXISTS core_audit_log (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor      TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    action     TEXT NOT NULL,
    ecosystem  TEXT,
    package    TEXT,
    version    TEXT,
    detail     JSONB NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS core_audit_log_package_idx ON core_audit_log (ecosystem, package, at DESC);
