use kube::CustomResourceExt;

fn main() {
    for crd in [
        serde_yaml::to_string(&waf_manager::WafBlock::crd()).unwrap(),
        serde_yaml::to_string(&waf_manager::WafPolicy::crd()).unwrap(),
    ] {
        println!("---");
        print!("{crd}");
    }
}
