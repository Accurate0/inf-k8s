use kanidm_sync::KanidmOAuth2Client;
use kube::CustomResourceExt;

fn main() {
    print!(
        "{}",
        serde_yaml::to_string(&KanidmOAuth2Client::crd()).unwrap()
    );
}
