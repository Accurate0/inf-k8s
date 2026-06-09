use std::{collections::HashSet, path::Path};
use yaml_include::Transformer;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rules_path = manifest_dir.join("config.yaml");

    let transformer = Transformer::new(rules_path, true)
        .expect("failed to load config.yaml for include processing");
    let resolved = transformer.to_string();

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("rules.merged.yaml"), &resolved)
        .expect("write merged rules");

    let schema_str =
        std::fs::read_to_string(manifest_dir.join("config.schema.json")).expect("read schema");
    std::fs::write(Path::new(&out_dir).join("config.schema.json"), &schema_str)
        .expect("write schema");

    // Validate rules against schema at build time
    let yaml_value: serde_json::Value =
        yaml_serde::from_str(&resolved).expect("config.yaml is not valid YAML");

    if std::env::var("SKIP_SCHEMA_VALIDATION").is_err() {
        let schema: serde_json::Value =
            serde_json::from_str(&schema_str).expect("config.schema.json is not valid JSON");
        let validator = jsonschema::validator_for(&schema)
            .expect("config.schema.json is not a valid JSON Schema");

        let errors: Vec<String> = validator
            .iter_errors(&yaml_value)
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

    // Validate rule dependencies at build time
    if let Some(rules) = yaml_value.get("rules").and_then(|r| r.as_array()) {
        let rule_names: HashSet<String> = rules
            .iter()
            .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();

        for rule in rules {
            let rule_name = rule
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("<unknown>");
            if let Some(deps) = rule.get("depends_on").and_then(|d| d.as_array()) {
                for dep in deps {
                    if let Some(dep_name) = dep.as_str() {
                        if !rule_names.contains(dep_name) {
                            panic!(
                                "rule '{rule_name}': depends_on references unknown rule '{dep_name}'"
                            );
                        }
                        if dep_name == rule_name {
                            panic!("rule '{rule_name}': depends_on cannot reference itself");
                        }
                    }
                }
            }
        }
    }

    // Validate matcher check refs at build time. Walk every node looking
    // for `{ref: name}` and check it resolves against the enclosing rule's
    // `checks:` or the top-level `checks:`.
    let global_checks: HashSet<String> = yaml_value
        .get("checks")
        .and_then(|f| f.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    if let Some(rules) = yaml_value.get("rules").and_then(|r| r.as_array()) {
        for rule in rules {
            let rule_name = rule
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("<unknown>");
            let local_checks: HashSet<String> = rule
                .get("checks")
                .and_then(|f| f.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();

            // The rule's `checks:` map itself can contain refs to other
            // checks (local or global); validate those too, but skip the
            // top-level keys of the `checks:` map (they are definitions).
            let scan_roots: Vec<&serde_json::Value> = ["when", "actions", "checks"]
                .iter()
                .filter_map(|k| rule.get(*k))
                .collect();

            for root in scan_roots {
                collect_and_check_refs(root, rule_name, &local_checks, &global_checks);
            }
        }
    }

    println!("cargo:rerun-if-changed=config.yaml");
    println!("cargo:rerun-if-changed=config.schema.json");
    println!("cargo:rerun-if-changed=rules/");
}

fn collect_and_check_refs(
    node: &serde_json::Value,
    rule_name: &str,
    locals: &HashSet<String>,
    globals: &HashSet<String>,
) {
    match node {
        serde_json::Value::Object(map) => {
            if let Some(ref_name) = map.get("ref").and_then(|v| v.as_str())
                && map.len() == 1
            {
                let ok = match ref_name.strip_prefix("global.") {
                    Some(g) => globals.contains(g),
                    None => locals.contains(ref_name),
                };
                if !ok {
                    panic!(
                        "rule '{rule_name}': matcher ref `{ref_name}` does not resolve to any defined check"
                    );
                }
                return;
            }
            for v in map.values() {
                collect_and_check_refs(v, rule_name, locals, globals);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_and_check_refs(v, rule_name, locals, globals);
            }
        }
        _ => {}
    }
}
