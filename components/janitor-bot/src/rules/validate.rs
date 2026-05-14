use super::expr;
use super::schema::{ActionsDef, RulesFile};
use anyhow::{Context, anyhow};
use std::collections::HashSet;

const RULES_YAML: &str = include_str!(concat!(env!("OUT_DIR"), "/rules.merged.yaml"));
const RULES_SCHEMA: &str = include_str!(concat!(env!("OUT_DIR"), "/rules.schema.json"));

pub fn load_and_validate_rules() -> anyhow::Result<RulesFile> {
    let yaml_value: serde_json::Value =
        yaml_serde::from_str(RULES_YAML).context("rules.yaml is not valid YAML")?;

    let schema: serde_json::Value =
        serde_json::from_str(RULES_SCHEMA).context("rules.schema.json is not valid JSON")?;

    let validator = jsonschema::validator_for(&schema)
        .context("rules.schema.json is not a valid JSON Schema")?;

    let errors: Vec<String> = validator
        .iter_errors(&yaml_value)
        .map(|e| format!("  at {}: {}", e.instance_path(), e))
        .collect();

    if !errors.is_empty() {
        return Err(anyhow!(
            "rules.yaml failed schema validation:\n{}",
            errors.join("\n")
        ));
    }

    let rules: RulesFile =
        yaml_serde::from_str(RULES_YAML).context("rules.yaml could not be deserialized")?;

    validate_expressions(&rules)?;

    Ok(rules)
}

fn validate_expressions(rules: &RulesFile) -> anyhow::Result<()> {
    for rule in &rules.rules {
        let ActionsDef::Conditional(groups) = &rule.actions else {
            continue;
        };

        let defined_vars: HashSet<String> = rule.variables.iter().map(|v| v.var.clone()).collect();

        for group in groups {
            let parsed = expr::parse(&group.when).map_err(|e| {
                anyhow!(
                    "rule '{}': invalid expression '{}': {e}",
                    rule.name,
                    group.when
                )
            })?;

            for var in expr::referenced_vars(&parsed) {
                if !defined_vars.contains(&var) {
                    return Err(anyhow!(
                        "rule '{}': expression '{}' references undefined variable '{var}'",
                        rule.name,
                        group.when
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rules(yaml: &str) -> RulesFile {
        yaml_serde::from_str(yaml).unwrap()
    }

    #[test]
    fn validate_flat_actions_passes() {
        let rules = make_rules(
            r#"
rules:
  - name: test
    enabled: true
    matches:
      type: forgejo
    actions:
      - type: approve
"#,
        );
        assert!(validate_expressions(&rules).is_ok());
    }

    #[test]
    fn validate_conditional_with_defined_vars() {
        let rules = make_rules(
            r#"
rules:
  - name: test
    enabled: true
    matches:
      type: forgejo
    variables:
      - var: foo
        type: is_open
      - var: bar
        type: has_conflicts
    actions:
      - when: "foo && !bar"
        run:
          - type: approve
"#,
        );
        assert!(validate_expressions(&rules).is_ok());
    }

    #[test]
    fn validate_undefined_variable_fails() {
        let rules = make_rules(
            r#"
rules:
  - name: bad-rule
    enabled: true
    matches:
      type: forgejo
    variables:
      - var: foo
        type: is_open
    actions:
      - when: "foo && missing_var"
        run:
          - type: approve
"#,
        );
        let err = validate_expressions(&rules).unwrap_err();
        assert!(err.to_string().contains("missing_var"));
        assert!(err.to_string().contains("bad-rule"));
    }

    #[test]
    fn validate_invalid_expression_fails() {
        let rules = make_rules(
            r#"
rules:
  - name: bad-expr
    enabled: true
    matches:
      type: forgejo
    variables: []
    actions:
      - when: "&& broken"
        run:
          - type: approve
"#,
        );
        let err = validate_expressions(&rules).unwrap_err();
        assert!(err.to_string().contains("bad-expr"));
    }

    #[test]
    fn validate_empty_when_passes() {
        let rules = make_rules(
            r#"
rules:
  - name: test
    enabled: true
    matches:
      type: forgejo
    variables: []
    actions:
      - when: ""
        run:
          - type: approve
"#,
        );
        assert!(validate_expressions(&rules).is_ok());
    }

    #[test]
    fn validate_multiple_groups_all_valid() {
        let rules = make_rules(
            r#"
rules:
  - name: test
    enabled: true
    matches:
      type: forgejo
    variables:
      - var: a
        type: is_open
      - var: b
        type: has_conflicts
    actions:
      - when: "a"
        run:
          - type: approve
      - when: "!a && b"
        run:
          - type: comment
            body: hi
"#,
        );
        assert!(validate_expressions(&rules).is_ok());
    }

    #[test]
    fn validate_second_group_has_undefined_var() {
        let rules = make_rules(
            r#"
rules:
  - name: test
    enabled: true
    matches:
      type: forgejo
    variables:
      - var: a
        type: is_open
    actions:
      - when: "a"
        run:
          - type: approve
      - when: "a && nope"
        run:
          - type: comment
            body: hi
"#,
        );
        let err = validate_expressions(&rules).unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn validate_no_variables_with_conditional_using_literals() {
        let rules = make_rules(
            r#"
rules:
  - name: test
    enabled: true
    matches:
      type: forgejo
    variables: []
    actions:
      - when: "true"
        run:
          - type: approve
"#,
        );
        assert!(validate_expressions(&rules).is_ok());
    }

    #[test]
    fn validate_multiple_rules_second_fails() {
        let rules = make_rules(
            r#"
rules:
  - name: good
    enabled: true
    matches:
      type: forgejo
    variables:
      - var: x
        type: is_open
    actions:
      - when: "x"
        run:
          - type: approve
  - name: bad
    enabled: true
    matches:
      type: forgejo
    variables: []
    actions:
      - when: "undefined"
        run:
          - type: approve
"#,
        );
        let err = validate_expressions(&rules).unwrap_err();
        assert!(err.to_string().contains("bad"));
        assert!(err.to_string().contains("undefined"));
    }

    #[test]
    fn validate_real_rules_load_successfully() {
        let result = load_and_validate_rules();
        assert!(result.is_ok(), "real rules failed: {}", result.unwrap_err());
    }
}
