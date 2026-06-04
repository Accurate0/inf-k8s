use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::{
    CustomResourceDefinition, ValidationRule,
};
use kanidm_sync::{KanidmGroup, KanidmOAuth2Client, KanidmUser};
use kube::CustomResourceExt;

fn main() {
    let mut oauth2 = KanidmOAuth2Client::crd();
    inject_spec_validations(
        &mut oauth2,
        &[
            ("self.name != \"\"", "spec.name must not be empty"),
            (
                "self.redirectUrls.size() > 0",
                "spec.redirectUrls must contain at least one URL",
            ),
        ],
    );

    let mut group = KanidmGroup::crd();
    inject_spec_validations(
        &mut group,
        &[("self.name != \"\"", "spec.name must not be empty")],
    );

    let mut user = KanidmUser::crd();
    inject_spec_validations(
        &mut user,
        &[("self.name != \"\"", "spec.name must not be empty")],
    );

    let docs: Vec<String> = [oauth2, group, user].into_iter().map(render).collect();
    print!("{}", docs.join("---\n"));
}

fn render(mut crd: CustomResourceDefinition) -> String {
    // The apiserver omits these empty collections; drop them so the committed
    // manifest matches the stored object and ArgoCD doesn't show perpetual drift.
    crd.spec.names.categories = None;
    crd.spec.names.short_names = None;
    serde_yaml::to_string(&crd).unwrap()
}

/// Inject CEL validation rules on the `spec` property of each CRD version.
fn inject_spec_validations(crd: &mut CustomResourceDefinition, rules: &[(&str, &str)]) {
    let validation_rules: Vec<ValidationRule> = rules
        .iter()
        .map(|(rule, message)| ValidationRule {
            rule: rule.to_string(),
            message: Some(message.to_string()),
            ..Default::default()
        })
        .collect();

    for version in &mut crd.spec.versions {
        if let Some(schema) = &mut version.schema {
            if let Some(open_api_schema) = &mut schema.open_api_v3_schema {
                if let Some(properties) = &mut open_api_schema.properties {
                    if let Some(spec_schema) = properties.get_mut("spec") {
                        spec_schema.x_kubernetes_validations = Some(validation_rules.clone());
                    }
                }
            }
        }
    }
}
