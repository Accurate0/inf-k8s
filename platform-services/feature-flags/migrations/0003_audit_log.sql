CREATE TABLE flag_changes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    version bigint NOT NULL,
    actor text NOT NULL,
    action text NOT NULL,
    target_kind text NOT NULL,
    target_key text NOT NULL,
    detail jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX flag_changes_target_idx ON flag_changes (target_kind, target_key, created_at DESC);
CREATE INDEX flag_changes_created_idx ON flag_changes (created_at DESC);
