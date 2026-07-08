//! Generate the JSON Schemas for the `ffctl config` YAML files from the Rust types
//! in `feature_flags::flag_config`. Run with `cargo run --bin gen-schema`; the
//! output under `config/schema` is what the `# yaml-language-server` lines point at.

use feature_flags::flag_config::{FlagDoc, SegmentsFile};
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/schema");

    write(&dir.join("flag.json"), schemars::schema_for!(FlagDoc));
    write(&dir.join("segments.json"), schemars::schema_for!(SegmentsFile));
}

fn write(path: &std::path::Path, schema: schemars::Schema) {
    let json = serde_json::to_string_pretty(&schema).expect("serialize schema");
    std::fs::write(path, format!("{json}\n")).expect("write schema");
    eprintln!("wrote {}", path.display());
}
