use kube::CustomResourceExt;

/// Emits both CRDs so `manifests/crd.yaml` stays derived from the Rust types
/// rather than hand-maintained.
fn main() {
    for crd in [
        serde_yaml::to_string(&waf_manager::WafBlock::crd()).unwrap(),
        serde_yaml::to_string(&waf_manager::WafPolicy::crd()).unwrap(),
    ] {
        println!("---");
        print!("{crd}");
    }
}
