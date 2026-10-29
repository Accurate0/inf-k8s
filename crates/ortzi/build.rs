fn main() {
    println!("cargo:rerun-if-changed=ortzi.yaml");
    let manifest = std::fs::read_to_string("ortzi.yaml").expect("reading ortzi.yaml");
    let version = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version:"))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .expect("ortzi.yaml has no package.version");
    println!("cargo:rustc-env=ORTZI_VERSION={version}");
}
