use std::path::Path;

use anyhow::Context;
use cynic::{MutationBuilder, QueryBuilder};
use graph_client::GraphClient;
use registry_api::{HarborAccess, HarborAction, HarborToken, Target};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::graphql as gql;

/// The registry, reached only through the graph.
pub struct Client {
    graph: GraphClient,
}

pub fn not_found(e: &anyhow::Error) -> bool {
    graph_client::error_code(e) == Some("not_found")
}

/// Turns a not_found error into `None`.
pub fn found<T>(result: anyhow::Result<T>) -> anyhow::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if not_found(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn target_in(target: Target) -> gql::Target {
    match target {
        Target::DarwinArm64 => gql::Target::DarwinArm64,
        Target::LinuxAmd64 => gql::Target::LinuxAmd64,
        Target::Wasm => gql::Target::Wasm,
    }
}

fn target_out(target: gql::Target) -> Target {
    match target {
        gql::Target::DarwinArm64 => Target::DarwinArm64,
        gql::Target::LinuxAmd64 => Target::LinuxAmd64,
        gql::Target::Wasm => Target::Wasm,
    }
}

pub fn artifact(artifact: gql::Artifact) -> registry_api::Artifact {
    registry_api::Artifact {
        target: target_out(artifact.target),
        sha256: artifact.sha256,
        size: artifact.size.into(),
    }
}

fn action_in(action: HarborAction) -> gql::HarborAction {
    match action {
        HarborAction::Pull => gql::HarborAction::Pull,
        HarborAction::Push => gql::HarborAction::Push,
    }
}

fn action_out(action: gql::HarborAction) -> HarborAction {
    match action {
        gql::HarborAction::Pull => HarborAction::Pull,
        gql::HarborAction::Push => HarborAction::Push,
    }
}

/// A registry graph id, from either the id itself or the bare key older output printed.
pub fn registry_id(node: &str, id: &str) -> cynic::Id {
    if id.starts_with("registry:") {
        cynic::Id::new(id)
    } else {
        cynic::Id::new(format!("registry:{node}:{id}"))
    }
}

impl Client {
    pub fn new(url: &str, token: String) -> Self {
        Self {
            graph: GraphClient::with_token(url, token),
        }
    }

    pub fn graph(&self) -> &GraphClient {
        &self.graph
    }

    pub async fn whoami(&self) -> anyhow::Result<gql::WhoAmI> {
        self.graph
            .run(gql::WhoAmIQuery::build(()))
            .await?
            .registry_whoami
            .context("the graph returned no registryWhoami")
    }

    pub async fn catalog(&self) -> anyhow::Result<registry_api::CatalogRef> {
        let current = self
            .graph
            .run(gql::CatalogQuery::build(()))
            .await?
            .registry_catalog
            .context("the graph returned no registryCatalog")?;
        Ok(registry_api::CatalogRef {
            seq: current.seq.into(),
            digest: current.digest,
        })
    }

    pub async fn harbor_credential(
        &self,
        access: Vec<HarborAccess>,
    ) -> anyhow::Result<HarborToken> {
        let access = access
            .into_iter()
            .map(|item| gql::HarborAccessInput {
                project: item.project,
                actions: item.actions.into_iter().map(action_in).collect(),
                package: item.package,
            })
            .collect();
        let credential = self
            .graph
            .run(gql::CreateHarborCredential::build(
                gql::HarborCredentialVariables { access },
            ))
            .await?
            .registry_create_harbor_credential;

        Ok(HarborToken {
            registry: credential.registry,
            username: credential.username,
            password: credential.password,
            expires_at: credential.expires_at,
            access: credential
                .access
                .into_iter()
                .map(|item| HarborAccess {
                    project: item.project,
                    actions: item.actions.into_iter().map(action_out).collect(),
                    package: item.package,
                })
                .collect(),
        })
    }

    /// Returns the revoked robot's username.
    pub async fn revoke_harbor_credential(&self, username: &str) -> anyhow::Result<String> {
        Ok(self
            .graph
            .run(gql::RevokeHarborCredential::build(gql::UsernameVariables {
                username: username.to_owned(),
            }))
            .await?
            .registry_revoke_harbor_credential)
    }

    pub async fn sign_image(
        &self,
        repository: &str,
        digest: &str,
    ) -> anyhow::Result<gql::ImageSignature> {
        Ok(self
            .graph
            .run(gql::SignImage::build(gql::SignImageVariables {
                repository: repository.to_owned(),
                digest: digest.to_owned(),
            }))
            .await?
            .registry_sign_image)
    }

    pub async fn version(
        &self,
        package: &str,
        version: &str,
    ) -> anyhow::Result<Option<gql::Version>> {
        found(
            self.graph
                .run(gql::VersionQuery::build(gql::VersionVariables {
                    package: package.to_owned(),
                    version: version.to_owned(),
                }))
                .await
                .map(|found| found.registry_version),
        )
        .map(Option::flatten)
    }

    pub async fn package(&self, name: &str) -> anyhow::Result<Option<gql::Package>> {
        found(
            self.graph
                .run(gql::PackageQuery::build(gql::NameVariables {
                    name: name.to_owned(),
                }))
                .await
                .map(|found| found.registry_package),
        )
        .map(Option::flatten)
    }
}

pub async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(8_637_072), "8.2 MiB");
        assert_eq!(human_size(177_960_386), "169.7 MiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
