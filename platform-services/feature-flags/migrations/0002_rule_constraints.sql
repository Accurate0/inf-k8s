CREATE TABLE rule_constraints (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_id uuid NOT NULL REFERENCES flag_rules (id) ON DELETE CASCADE,
    group_index integer NOT NULL DEFAULT 0,
    attribute text NOT NULL,
    operator text NOT NULL,
    values jsonb NOT NULL DEFAULT '[]'::jsonb
);

CREATE TRIGGER bump_on_rule_constraints
AFTER INSERT OR UPDATE OR DELETE ON rule_constraints
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();
