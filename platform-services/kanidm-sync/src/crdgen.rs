use kanidm_sync::KanidmOAuth2Client;
use kube::CustomResourceExt;

fn main() {
    let mut crd = KanidmOAuth2Client::crd();
    // The apiserver omits these empty collections; drop them so the committed
    // manifest matches the stored object and ArgoCD doesn't show perpetual drift.
    crd.spec.names.categories = None;
    crd.spec.names.short_names = None;
    for version in &mut crd.spec.versions {
        version.additional_printer_columns = None;
    }
    print!("{}", serde_yaml::to_string(&crd).unwrap());
}
