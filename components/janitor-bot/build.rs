use std::path::Path;
use yaml_include::Transformer;

#[allow(dead_code)]
#[path = "src/rules/expr.rs"]
mod expr;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rules_path = manifest_dir.join("rules.yaml");

    let transformer = Transformer::new(rules_path, true)
        .expect("failed to load rules.yaml for include processing");
    let resolved = transformer.to_string();

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("rules.merged.yaml"), &resolved)
        .expect("write merged rules");

    let schema_str =
        std::fs::read_to_string(manifest_dir.join("rules.schema.json")).expect("read schema");
    std::fs::write(Path::new(&out_dir).join("rules.schema.json"), &schema_str)
        .expect("write schema");

    // Validate rules against schema at build time
    let yaml_value: serde_json::Value =
        yaml_serde::from_str(&resolved).expect("rules.yaml is not valid YAML");
    let schema: serde_json::Value =
        serde_json::from_str(&schema_str).expect("rules.schema.json is not valid JSON");
    let validator =
        jsonschema::validator_for(&schema).expect("rules.schema.json is not a valid JSON Schema");

    let errors: Vec<String> = validator
        .iter_errors(&yaml_value)
        .map(|e| format!("  at {}: {}", e.instance_path(), e))
        .collect();

    if !errors.is_empty() {
        panic!(
            "rules.yaml failed schema validation:\n{}",
            errors.join("\n")
        );
    }

    // Validate expressions at build time
    if let Some(rules) = yaml_value.get("rules").and_then(|r| r.as_array()) {
        for rule in rules {
            let rule_name = rule
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("<unknown>");
            let defined_vars: std::collections::HashSet<String> = rule
                .get("variables")
                .and_then(|v| v.as_array())
                .map(|vars| {
                    vars.iter()
                        .filter_map(|v| v.get("var").and_then(|s| s.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let groups = match rule.get("actions").and_then(|a| a.as_array()) {
                Some(actions) => actions
                    .iter()
                    .filter_map(|a| a.get("when").and_then(|w| w.as_str()))
                    .collect::<Vec<_>>(),
                None => continue,
            };

            for when in groups {
                let parsed = match expr::parse(when) {
                    Ok(p) => p,
                    Err(e) => panic!("rule '{rule_name}': invalid expression '{when}': {e}"),
                };
                for var in expr::referenced_vars(&parsed) {
                    if !defined_vars.contains(&var) {
                        panic!(
                            "rule '{rule_name}': expression '{when}' references undefined variable '{var}'"
                        );
                    }
                }
            }
        }
    }

    println!("cargo:rerun-if-changed=rules.yaml");
    println!("cargo:rerun-if-changed=rules.schema.json");
    println!("cargo:rerun-if-changed=rules/");
}
