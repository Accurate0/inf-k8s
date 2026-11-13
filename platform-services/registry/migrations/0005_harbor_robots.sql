CREATE TABLE IF NOT EXISTS core_harbor_robots (
    id         BIGINT PRIMARY KEY,
    username   TEXT NOT NULL UNIQUE,
    actor      TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    access     JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS core_harbor_robots_live_idx ON core_harbor_robots (expires_at) WHERE deleted_at IS NULL;
