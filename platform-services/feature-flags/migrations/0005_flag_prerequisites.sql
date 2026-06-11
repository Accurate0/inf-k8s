CREATE TABLE flag_prerequisites (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_id uuid NOT NULL REFERENCES flags (id) ON DELETE CASCADE,
    prerequisite_key text NOT NULL,
    variant_key text NOT NULL,
    UNIQUE (flag_id, prerequisite_key)
);

CREATE TRIGGER bump_on_flag_prerequisites
AFTER INSERT OR UPDATE OR DELETE ON flag_prerequisites
FOR EACH STATEMENT EXECUTE FUNCTION ff_bump_version();
