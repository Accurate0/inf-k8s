use crate::argocd::ArgocdClient;
use crate::feature_flag::FeatureFlagClient;
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;
use crate::llm::LlmClient;
use crate::registry::RegistryClient;

pub struct Clients {
    pub forgejo: ForgejoClient,
    pub github: GitHubClient,
    pub argocd: ArgocdClient,
    pub feature_flag: FeatureFlagClient,
    pub registry: RegistryClient,
    /// Present only when `AI_GATEWAY_TOKEN` is configured; gates the autofix command.
    pub llm: Option<LlmClient>,
}

impl Clients {
    pub fn new(
        forgejo: ForgejoClient,
        github: GitHubClient,
        argocd: ArgocdClient,
        feature_flag: FeatureFlagClient,
        registry: RegistryClient,
        llm: Option<LlmClient>,
    ) -> Self {
        Self {
            forgejo,
            github,
            argocd,
            feature_flag,
            registry,
            llm,
        }
    }
}
