CREATE TABLE IF NOT EXISTS core_promotions (
    id            BIGSERIAL PRIMARY KEY,
    app           TEXT NOT NULL,
    repository    TEXT NOT NULL,
    digest        TEXT NOT NULL,
    sha           TEXT,
    committed_at  TIMESTAMPTZ,
    promoted_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor         TEXT NOT NULL,
    actor_kind    TEXT NOT NULL,
    forced        BOOLEAN NOT NULL DEFAULT false,
    dispatched_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS core_promotions_repository
    ON core_promotions (repository, promoted_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS core_promotions_undispatched
    ON core_promotions (id) WHERE dispatched_at IS NULL;
