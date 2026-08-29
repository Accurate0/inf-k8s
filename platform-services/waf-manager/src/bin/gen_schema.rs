use std::path::PathBuf;
use waf_manager::config::{Config, WorkflowDef};

fn main() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let config_schema = schemars::schema_for!(Config);
    let json = serde_json::to_string_pretty(&config_schema).expect("serialize schema");
    let out = base.join("config.schema.json");

    std::fs::write(&out, format!("{json}\n")).expect("write schema");
    eprintln!("wrote {}", out.display());

    let workflow_schema = schemars::schema_for!(WorkflowDef);
    let json = serde_json::to_string_pretty(&workflow_schema).expect("serialize schema");
    let out = base.join("workflow.schema.json");

    std::fs::write(&out, format!("{json}\n")).expect("write schema");
    eprintln!("wrote {}", out.display());
}
