CREATE TABLE IF NOT EXISTS core_graph_exchanges (
    jti        TEXT PRIMARY KEY,
    account    TEXT NOT NULL,
    actor      TEXT NOT NULL,
    forgejo    JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
