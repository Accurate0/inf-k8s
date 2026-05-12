use janitor_bot::schema::RulesFile;
use std::path::PathBuf;

fn main() {
    let schema = schemars::schema_for!(RulesFile);
    let json = serde_json::to_string_pretty(&schema).expect("serialize schema");

    let out = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rules.schema.json")
    });

    std::fs::write(&out, format!("{json}\n")).expect("write schema");
    eprintln!("wrote {}", out.display());
}
