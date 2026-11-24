CREATE TABLE IF NOT EXISTS core_images (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repository        TEXT NOT NULL,
    digest            TEXT NOT NULL,
    source_repository TEXT,
    source_ref        TEXT,
    source_sha        TEXT,
    source_run_id     TEXT,
    workflow_ref      TEXT,
    signed_by         TEXT NOT NULL,
    signed_by_kind    TEXT NOT NULL,
    signed_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    attested_at       TIMESTAMPTZ,
    UNIQUE (repository, digest)
);

CREATE INDEX IF NOT EXISTS core_images_repository_idx
    ON core_images (repository, signed_at DESC, id DESC);

INSERT INTO core_images (repository, digest, signed_by, signed_by_kind, signed_at)
SELECT DISTINCT ON (detail->>'repository', detail->>'digest')
       detail->>'repository', detail->>'digest', actor, actor_kind, at
FROM core_audit_log
WHERE action = 'harbor.sign' AND detail ? 'repository' AND detail ? 'digest'
ORDER BY detail->>'repository', detail->>'digest', at
ON CONFLICT DO NOTHING;
