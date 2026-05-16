use crate::argocd::ArgocdClient;
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;

pub struct Clients {
    pub forgejo: ForgejoClient,
    pub github: GitHubClient,
    pub argocd: ArgocdClient,
}

impl Clients {
    pub fn new(forgejo: ForgejoClient, github: GitHubClient, argocd: ArgocdClient) -> Self {
        Self {
            forgejo,
            github,
            argocd,
        }
    }
}
