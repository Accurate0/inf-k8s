use crate::argocd::ArgocdClient;
use crate::feature_flag::FeatureFlagClient;
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;
use crate::registry::RegistryClient;

pub struct Clients {
    pub forgejo: ForgejoClient,
    pub github: GitHubClient,
    pub argocd: ArgocdClient,
    pub feature_flag: FeatureFlagClient,
    pub registry: RegistryClient,
}

impl Clients {
    pub fn new(
        forgejo: ForgejoClient,
        github: GitHubClient,
        argocd: ArgocdClient,
        feature_flag: FeatureFlagClient,
        registry: RegistryClient,
    ) -> Self {
        Self {
            forgejo,
            github,
            argocd,
            feature_flag,
            registry,
        }
    }
}
