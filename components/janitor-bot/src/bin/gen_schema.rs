use janitor_bot::rules::schema::{RuleDef, RulesFile};
use std::path::PathBuf;

fn main() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let rules_schema = schemars::schema_for!(RulesFile);
    let json = serde_json::to_string_pretty(&rules_schema).expect("serialize schema");
    let out = base.join("rules.schema.json");

    std::fs::write(&out, format!("{json}\n")).expect("write schema");
    eprintln!("wrote {}", out.display());

    let rule_schema = schemars::schema_for!(RuleDef);
    let json = serde_json::to_string_pretty(&rule_schema).expect("serialize schema");
    let out = base.join("rule.schema.json");

    std::fs::write(&out, format!("{json}\n")).expect("write schema");
    eprintln!("wrote {}", out.display());
}
