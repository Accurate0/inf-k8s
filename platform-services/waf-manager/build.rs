use std::path::Path;
use yaml_include::Transformer;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config_path = manifest_dir.join("config.yaml");

    let transformer = Transformer::new(config_path, true)
        .expect("failed to load config.yaml for include processing");
    let resolved = transformer.to_string();

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("config.merged.yaml"), &resolved)
        .expect("write merged config");

    let value: serde_json::Value =
        serde_yaml::from_str(&resolved).expect("config.yaml is not valid YAML");

    if std::env::var("SKIP_SCHEMA_VALIDATION").is_err() {
        let schema_str =
            std::fs::read_to_string(manifest_dir.join("config.schema.json")).expect("read schema");
        let schema: serde_json::Value =
            serde_json::from_str(&schema_str).expect("config.schema.json is not valid JSON");
        let validator = jsonschema::validator_for(&schema)
            .expect("config.schema.json is not a valid JSON Schema");

        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| format!("  at {}: {}", e.instance_path(), e))
            .collect();

        if !errors.is_empty() {
            panic!(
                "config.yaml failed schema validation:\n{}",
                errors.join("\n")
            );
        }
    } else {
        println!("cargo:warning=Skipping schema validation (SKIP_SCHEMA_VALIDATION set)");
    }

    // Duplicate names would make the /workflows page and the block `reason`
    // ambiguous about which workflow fired.
    if let Some(workflows) = value.get("workflows").and_then(|w| w.as_array()) {
        let mut seen = std::collections::HashSet::new();

        for workflow in workflows {
            let name = workflow
                .get("name")
                .and_then(|n| n.as_str())
                .expect("every workflow needs a name");

            if !seen.insert(name) {
                panic!("duplicate workflow name '{name}'");
            }

            let declared: std::collections::HashSet<&str> = workflow
                .get("signals")
                .and_then(|s| s.as_object())
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();

            if let Some(when) = workflow.get("when") {
                check_signal_refs(when, name, &declared);
            }
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=config.yaml");
    println!("cargo:rerun-if-changed=config.schema.json");
    println!("cargo:rerun-if-changed=workflows/");
}

/// Walks a matcher tree and fails the build on a `signal` matcher naming a query
/// the workflow does not declare — a typo would otherwise silently evaluate to
/// zero and the workflow would never fire.
fn check_signal_refs(
    node: &serde_json::Value,
    workflow: &str,
    declared: &std::collections::HashSet<&str>,
) {
    match node {
        serde_json::Value::Object(map) => {
            if map.get("type").and_then(|t| t.as_str()) == Some("signal") {
                let name = map
                    .get("name")
                    .and_then(|n| n.as_str())
                    .expect("a signal matcher needs a name");

                if !declared.contains(name) {
                    panic!("workflow '{workflow}': matcher references undeclared signal '{name}'");
                }

                return;
            }

            for value in map.values() {
                check_signal_refs(value, workflow, declared);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                check_signal_refs(item, workflow, declared);
            }
        }
        _ => {}
    }
}
