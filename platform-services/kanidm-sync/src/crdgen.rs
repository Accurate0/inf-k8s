use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kanidm_sync::{KanidmGroup, KanidmOAuth2Client, KanidmUser};
use kube::CustomResourceExt;

fn main() {
    let crds = [
        KanidmOAuth2Client::crd(),
        KanidmGroup::crd(),
        KanidmUser::crd(),
    ];
    let docs: Vec<String> = crds.into_iter().map(render).collect();
    print!("{}", docs.join("---\n"));
}

fn render(mut crd: CustomResourceDefinition) -> String {
    // The apiserver omits these empty collections; drop them so the committed
    // manifest matches the stored object and ArgoCD doesn't show perpetual drift.
    crd.spec.names.categories = None;
    crd.spec.names.short_names = None;
    for version in &mut crd.spec.versions {
        version.additional_printer_columns = None;
    }
    serde_yaml::to_string(&crd).unwrap()
}
