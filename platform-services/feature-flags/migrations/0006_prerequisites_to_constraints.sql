-- Prerequisites are now expressed as `flag_matches` rule constraints rather than a
-- separate concept. Fold each existing prerequisite into every rule of its flag as a
-- new AND-group constraint, then drop the table.
INSERT INTO rule_constraints (rule_id, group_index, attribute, operator, values)
SELECT r.id,
       COALESCE(
           (SELECT MAX(rc.group_index) FROM rule_constraints rc WHERE rc.rule_id = r.id),
           -1
       ) + row_number() OVER (PARTITION BY r.id ORDER BY p.prerequisite_key),
       p.prerequisite_key,
       'flag_matches',
       to_jsonb(ARRAY[p.variant_key])
FROM flag_prerequisites p
JOIN flag_rules r ON r.flag_id = p.flag_id;

DROP TABLE flag_prerequisites;
