use std::path::Path;
use yaml_include::Transformer;

fn main() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rules_path = manifest_dir.join("rules.yaml");

    let transformer = Transformer::new(rules_path, true)
        .expect("failed to load rules.yaml for include processing");
    let resolved = transformer.to_string();

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("rules.merged.yaml"), &resolved)
        .expect("write merged rules");

    let schema =
        std::fs::read_to_string(manifest_dir.join("rules.schema.json")).expect("read schema");
    std::fs::write(Path::new(&out_dir).join("rules.schema.json"), &schema).expect("write schema");

    println!("cargo:rerun-if-changed=rules.yaml");
    println!("cargo:rerun-if-changed=rules.schema.json");
    println!("cargo:rerun-if-changed=rules/");
}
