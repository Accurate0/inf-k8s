use crate::argocd::ArgocdClient;
use crate::feature_flag::FeatureFlagClient;
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;

pub struct Clients {
    pub forgejo: ForgejoClient,
    pub github: GitHubClient,
    pub argocd: ArgocdClient,
    pub feature_flag: FeatureFlagClient,
}

impl Clients {
    pub fn new(
        forgejo: ForgejoClient,
        github: GitHubClient,
        argocd: ArgocdClient,
        feature_flag: FeatureFlagClient,
    ) -> Self {
        Self {
            forgejo,
            github,
            argocd,
            feature_flag,
        }
    }
}
