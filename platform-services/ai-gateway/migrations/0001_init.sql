CREATE EXTENSION IF NOT EXISTS timescaledb;

CREATE TABLE IF NOT EXISTS virtual_keys (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                 TEXT NOT NULL,
    key_hash             TEXT NOT NULL UNIQUE,
    allowed_models       TEXT[] NOT NULL DEFAULT '{}',
    monthly_token_budget BIGINT,
    revoked              BOOLEAN NOT NULL DEFAULT FALSE,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at         TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS virtual_keys_name_idx ON virtual_keys (name);

-- usage_events is a TimescaleDB hypertable: append-only, one row per request, always
-- queried by time range. No PRIMARY KEY / foreign key, matching the project's other
-- hypertables (a unique constraint would have to include the partition column, and a
-- FK on the hot insert path is undesirable); key_name is denormalised for querying.
CREATE TABLE IF NOT EXISTS usage_events (
    id              BIGINT GENERATED ALWAYS AS IDENTITY,
    key_id          UUID,
    key_name        TEXT NOT NULL,
    provider        TEXT NOT NULL,
    requested_model TEXT NOT NULL,
    resolved_model  TEXT NOT NULL,
    input_tokens    BIGINT NOT NULL DEFAULT 0,
    output_tokens   BIGINT NOT NULL DEFAULT 0,
    cache_hit       BOOLEAN NOT NULL DEFAULT FALSE,
    latency_ms      BIGINT NOT NULL DEFAULT 0,
    status          INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
) WITH (
    tsdb.hypertable,
    tsdb.partition_column = 'created_at',
    tsdb.orderby = 'created_at DESC'
);

CREATE INDEX IF NOT EXISTS usage_events_key_id_idx ON usage_events (key_id, created_at DESC);
CREATE INDEX IF NOT EXISTS usage_events_key_name_idx ON usage_events (key_name, created_at DESC);

-- Rolled up by a background task so Grafana queries stay cheap as usage_events grows.
CREATE TABLE IF NOT EXISTS usage_daily (
    key_name TEXT NOT NULL,
    model    TEXT NOT NULL,
    day      DATE NOT NULL,
    input_tokens  BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    requests      BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (key_name, model, day)
);
