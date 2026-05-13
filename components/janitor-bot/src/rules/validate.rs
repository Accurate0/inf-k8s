use super::schema::RulesFile;
use anyhow::{Context, anyhow};

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
    Ok(rules)
}
