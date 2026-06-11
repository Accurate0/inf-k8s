CREATE TABLE flags (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    key text NOT NULL UNIQUE,
    value_type text NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    default_variant_key text NOT NULL,
    archived boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE variants (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_id uuid NOT NULL REFERENCES flags (id) ON DELETE CASCADE,
    key text NOT NULL,
    value jsonb NOT NULL,
    UNIQUE (flag_id, key)
);

CREATE TABLE segments (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    key text NOT NULL UNIQUE,
    name text NOT NULL DEFAULT ''
);

CREATE TABLE segment_constraints (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    segment_id uuid NOT NULL REFERENCES segments (id) ON DELETE CASCADE,
    attribute text NOT NULL,
    operator text NOT NULL,
    values jsonb NOT NULL DEFAULT '[]'::jsonb
);

CREATE TABLE flag_rules (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_id uuid NOT NULL REFERENCES flags (id) ON DELETE CASCADE,
    rank integer NOT NULL,
    segment_key text,
    variant_key text,
    UNIQUE (flag_id, rank)
);

CREATE TABLE rule_distributions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_id uuid NOT NULL REFERENCES flag_rules (id) ON DELETE CASCADE,
    variant_key text NOT NULL,
    weight integer NOT NULL CHECK (weight >= 0 AND weight <= 100)
);

CREATE TABLE config_version (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    version bigint NOT NULL DEFAULT 1
);

INSERT INTO config_version (id, version) VALUES (true, 1);

CREATE FUNCTION ff_bump_version() RETURNS trigger AS $$
DECLARE
    v bigint;
BEGIN
    UPDATE config_version SET version = version + 1 WHERE id RETURNING version INTO v;
    PERFORM pg_notify('flag_changes', v::text);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER bump_on_flags
AFTER INSERT OR UPDATE OR DELETE ON flags
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();

CREATE TRIGGER bump_on_variants
AFTER INSERT OR UPDATE OR DELETE ON variants
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();

CREATE TRIGGER bump_on_segments
AFTER INSERT OR UPDATE OR DELETE ON segments
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();

CREATE TRIGGER bump_on_segment_constraints
AFTER INSERT OR UPDATE OR DELETE ON segment_constraints
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();

CREATE TRIGGER bump_on_flag_rules
AFTER INSERT OR UPDATE OR DELETE ON flag_rules
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();

CREATE TRIGGER bump_on_rule_distributions
AFTER INSERT OR UPDATE OR DELETE ON rule_distributions
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();
