use std::{fmt::Write, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=ortzi.yaml");
    println!("cargo:rerun-if-changed=keys");

    let manifest = std::fs::read_to_string("ortzi.yaml").expect("reading ortzi.yaml");
    let version = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version:"))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .expect("ortzi.yaml has no package.version");
    println!("cargo:rustc-env=ORTZI_VERSION={version}");

    let mut keys: Vec<_> = std::fs::read_dir("keys")
        .expect("reading keys/")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "pem"))
        .collect();
    keys.sort();
    assert!(
        !keys.is_empty(),
        "keys/ has no trusted registry public keys"
    );

    let mut source = String::from("pub const TRUSTED_KEYS: &[&str] = &[\n");
    for key in keys {
        let path = std::fs::canonicalize(&key).unwrap();
        writeln!(
            source,
            "    include_str!({:?}),",
            path.display().to_string()
        )
        .unwrap();
    }
    source.push_str("];\n");

    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("trusted_keys.rs");
    std::fs::write(out, source).unwrap();
}
