use crate::feature_flag::FeatureFlagClient;
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;
use crate::harbor::HarborClient;
use crate::registry::RegistryClient;

pub struct Clients {
    pub forgejo: ForgejoClient,
    pub github: GitHubClient,
    pub feature_flag: FeatureFlagClient,
    pub registry: RegistryClient,
    pub harbor: HarborClient,
}

impl Clients {
    pub fn new(
        forgejo: ForgejoClient,
        github: GitHubClient,
        feature_flag: FeatureFlagClient,
        registry: RegistryClient,
        harbor: HarborClient,
    ) -> Self {
        Self {
            forgejo,
            github,
            feature_flag,
            registry,
            harbor,
        }
    }
}
