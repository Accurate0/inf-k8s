use async_graphql::{
    ComplexObject, Context, Enum, ID, Interface, Json, Object, Result, SimpleObject,
};
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use registry_api::{ECOSYSTEM_CLI, Target};
use uuid::Uuid;

use super::{Gql, IDS, principal, state};
use crate::cli::{artifacts, packages};
use crate::core::publishers;

#[derive(Interface)]
#[allow(clippy::large_enum_variant)]
#[graphql(field(name = "id", ty = "ID"))]
pub enum Node {
    Package(Package),
    Version(Version),
    Artifact(Artifact),
    PublishToken(PublishToken),
    TrustedPublisher(TrustedPublisher),
    AuditEntry(AuditEntry),
    SigningKey(SigningKey),
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "RegistryTarget")]
pub enum RegistryTarget {
    DarwinArm64,
    LinuxAmd64,
    Wasm,
}

impl From<Target> for RegistryTarget {
    fn from(target: Target) -> Self {
        match target {
            Target::DarwinArm64 => Self::DarwinArm64,
            Target::LinuxAmd64 => Self::LinuxAmd64,
            Target::Wasm => Self::Wasm,
        }
    }
}

impl From<RegistryTarget> for Target {
    fn from(target: RegistryTarget) -> Self {
        match target {
            RegistryTarget::DarwinArm64 => Self::DarwinArm64,
            RegistryTarget::LinuxAmd64 => Self::LinuxAmd64,
            RegistryTarget::Wasm => Self::Wasm,
        }
    }
}

pub struct Package(pub registry_api::Package);

#[Object(name = "RegistryPackage")]
impl Package {
    async fn id(&self) -> ID {
        IDS.encode("package", &self.0.name)
    }

    async fn name(&self) -> &str {
        &self.0.name
    }

    async fn description(&self) -> &str {
        &self.0.description
    }

    async fn binary_name(&self) -> &str {
        &self.0.binary_name
    }

    async fn aliases(&self) -> &[String] {
        &self.0.aliases
    }

    async fn latest_version(&self, ctx: &Context<'_>) -> Result<Option<Version>> {
        match &self.0.latest {
            Some(version) => Version::load(ctx, &self.0.name, version).await.map(Some),
            None => Ok(None),
        }
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.0.created_at
    }

    async fn archived(&self) -> bool {
        self.0.archived
    }

    /// Published versions, newest first.
    async fn versions(&self, ctx: &Context<'_>) -> Result<Vec<Version>> {
        let detail = packages::get(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path(self.0.name.clone()),
        )
        .await
        .gql()?
        .0;

        let mut versions = Vec::with_capacity(detail.versions.len());
        for summary in detail.versions {
            versions.push(Version::load(ctx, &self.0.name, &summary.version).await?);
        }
        Ok(versions)
    }

    async fn version(&self, ctx: &Context<'_>, version: String) -> Result<Version> {
        Version::load(ctx, &self.0.name, &version).await
    }

    async fn trusted_publishers(&self, ctx: &Context<'_>) -> Result<Vec<TrustedPublisher>> {
        TrustedPublisher::list(ctx, Some(self.0.name.clone())).await
    }
}

impl Package {
    pub async fn load(ctx: &Context<'_>, name: &str) -> Result<Self> {
        let detail = packages::get(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path(name.to_owned()),
        )
        .await
        .gql()?;
        Ok(Self(detail.0.package))
    }

    pub async fn list(ctx: &Context<'_>, query: Option<String>) -> Result<Vec<Self>> {
        let packages = packages::list(
            State(state(ctx).clone()),
            principal(ctx)?,
            Query(packages::ListQuery { q: query }),
        )
        .await
        .gql()?;
        Ok(packages.0.into_iter().map(Self).collect())
    }
}

pub struct Version(pub registry_api::Version);

#[Object(name = "RegistryVersion")]
impl Version {
    async fn id(&self) -> ID {
        IDS.encode("version", version_key(&self.0.package, &self.0.version))
    }

    async fn package(&self, ctx: &Context<'_>) -> Result<Package> {
        Package::load(ctx, &self.0.package).await
    }

    async fn version(&self) -> &str {
        &self.0.version
    }

    async fn status(&self) -> &str {
        &self.0.status
    }

    async fn targets(&self) -> Vec<RegistryTarget> {
        self.0.targets.iter().copied().map(Into::into).collect()
    }

    async fn description(&self) -> &str {
        &self.0.description
    }

    async fn binary_name(&self) -> &str {
        &self.0.binary_name
    }

    async fn aliases(&self) -> &[String] {
        &self.0.aliases
    }

    async fn artifacts(&self) -> Vec<Artifact> {
        self.0
            .artifacts
            .iter()
            .map(|artifact| Artifact {
                package: self.0.package.clone(),
                version: self.0.version.clone(),
                artifact: artifact.clone(),
            })
            .collect()
    }

    async fn signed(&self) -> bool {
        self.0.signatures.is_some()
    }

    async fn published_at(&self) -> Option<DateTime<Utc>> {
        self.0.published_at
    }

    async fn published_by(&self) -> Option<&str> {
        self.0.published_by.as_deref()
    }

    async fn source(&self) -> Option<Source> {
        self.0.source.clone().map(|source| Source {
            repository: source.repository,
            git_ref: source.git_ref,
            sha: source.sha,
            run_id: source.run_id,
        })
    }

    async fn yanked(&self) -> Option<Yanked> {
        self.0.yanked.clone().map(|yanked| Yanked {
            at: yanked.at,
            by: yanked.by,
            reason: yanked.reason,
        })
    }
}

impl Version {
    pub async fn load(ctx: &Context<'_>, package: &str, version: &str) -> Result<Self> {
        let version = crate::cli::versions::get(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path((package.to_owned(), version.to_owned())),
        )
        .await
        .gql()?;
        Ok(Self(version.0))
    }
}

pub fn version_key(package: &str, version: &str) -> String {
    format!("{package}@{version}")
}

pub fn split_version_key(key: &str) -> Result<(&str, &str)> {
    key.split_once('@')
        .ok_or_else(|| async_graphql::Error::new(format!("invalid version key {key:?}")))
}

#[derive(SimpleObject)]
#[graphql(name = "RegistrySource")]
pub struct Source {
    repository: Option<String>,
    git_ref: Option<String>,
    sha: Option<String>,
    run_id: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryYanked")]
pub struct Yanked {
    at: DateTime<Utc>,
    by: String,
    reason: Option<String>,
}

pub struct Artifact {
    pub package: String,
    pub version: String,
    pub artifact: registry_api::Artifact,
}

#[Object(name = "RegistryArtifact")]
impl Artifact {
    async fn id(&self) -> ID {
        IDS.encode(
            "artifact",
            format!(
                "{}/{}",
                version_key(&self.package, &self.version),
                self.artifact.target.as_str()
            ),
        )
    }

    async fn target(&self) -> RegistryTarget {
        self.artifact.target.into()
    }

    async fn sha256(&self) -> &str {
        &self.artifact.sha256
    }

    async fn size(&self) -> i64 {
        self.artifact.size
    }

    /// A short-lived presigned download link, when the storage backend can sign one.
    async fn download_url(&self, ctx: &Context<'_>) -> Result<Option<DownloadUrl>> {
        let result = artifacts::download_url(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path((
                self.package.clone(),
                self.version.clone(),
                self.artifact.target.as_str().to_owned(),
            )),
        )
        .await;

        match result {
            Ok(url) => Ok(Some(DownloadUrl {
                url: url.0.url,
                expires_at: url.0.expires_at,
            })),
            Err(crate::error::ApiError::NotFound(_)) => Ok(None),
            Err(e) => Err(e).gql(),
        }
    }
}

impl Artifact {
    pub async fn load(ctx: &Context<'_>, key: &str) -> Result<Self> {
        let invalid = || async_graphql::Error::new(format!("invalid artifact key {key:?}"));
        let (version_key, target) = key.rsplit_once('/').ok_or_else(invalid)?;
        let (package, version) = split_version_key(version_key)?;
        let target: Target = serde_json::from_value(serde_json::Value::String(target.to_owned()))
            .map_err(|_| invalid())?;

        let loaded = Version::load(ctx, package, version).await?;
        let artifact = loaded
            .0
            .artifacts
            .into_iter()
            .find(|a| a.target == target)
            .ok_or_else(|| async_graphql::Error::new(format!("{key} not found")))?;
        Ok(Self {
            package: package.to_owned(),
            version: loaded.0.version,
            artifact,
        })
    }
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryDownloadUrl")]
pub struct DownloadUrl {
    url: String,
    expires_at: DateTime<Utc>,
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryScope")]
pub struct Scope {
    ecosystem: String,
    package: String,
}

pub struct PublishToken(pub registry_api::TokenInfo);

#[Object(name = "RegistryPublishToken")]
impl PublishToken {
    async fn id(&self) -> ID {
        IDS.encode("publishToken", self.0.id)
    }

    async fn name(&self) -> &str {
        &self.0.name
    }

    async fn prefix(&self) -> &str {
        &self.0.prefix
    }

    async fn scopes(&self) -> Vec<Scope> {
        self.0
            .scopes
            .iter()
            .map(|s| Scope {
                ecosystem: s.ecosystem.clone(),
                package: s.package.clone(),
            })
            .collect()
    }

    async fn created_by(&self) -> &str {
        &self.0.created_by
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.0.created_at
    }

    async fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.0.expires_at
    }

    async fn last_used_at(&self) -> Option<DateTime<Utc>> {
        self.0.last_used_at
    }

    async fn revoked_at(&self) -> Option<DateTime<Utc>> {
        self.0.revoked_at
    }
}

impl PublishToken {
    pub async fn list(ctx: &Context<'_>) -> Result<Vec<Self>> {
        let tokens = crate::core::tokens::list(State(state(ctx).clone()), principal(ctx)?)
            .await
            .gql()?;
        Ok(tokens.0.into_iter().map(Self).collect())
    }

    pub async fn load(ctx: &Context<'_>, id: Uuid) -> Result<Self> {
        Self::list(ctx)
            .await?
            .into_iter()
            .find(|t| t.0.id == id)
            .ok_or_else(|| async_graphql::Error::new(format!("token {id} not found")))
    }
}

pub struct TrustedPublisher(pub registry_api::TrustedPublisher);

#[Object(name = "RegistryTrustedPublisher")]
impl TrustedPublisher {
    async fn id(&self) -> ID {
        IDS.encode("trustedPublisher", self.0.id)
    }

    async fn ecosystem(&self) -> &str {
        &self.0.ecosystem
    }

    async fn package(&self, ctx: &Context<'_>) -> Result<Option<Package>> {
        if self.0.ecosystem != ECOSYSTEM_CLI {
            return Ok(None);
        }
        Package::load(ctx, &self.0.package).await.map(Some)
    }

    async fn provider(&self) -> &str {
        &self.0.provider
    }

    async fn issuer(&self) -> &str {
        &self.0.issuer
    }

    async fn repository(&self) -> &str {
        &self.0.repository
    }

    async fn repository_id(&self) -> Option<&str> {
        self.0.repository_id.as_deref()
    }

    async fn ref_pattern(&self) -> Option<&str> {
        self.0.ref_pattern.as_deref()
    }

    async fn workflow(&self) -> Option<&str> {
        self.0.workflow.as_deref()
    }

    async fn created_by(&self) -> &str {
        &self.0.created_by
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.0.created_at
    }
}

impl TrustedPublisher {
    pub async fn list(ctx: &Context<'_>, package: Option<String>) -> Result<Vec<Self>> {
        let publishers = publishers::list(
            State(state(ctx).clone()),
            principal(ctx)?,
            Query(publishers::ListQuery {
                ecosystem: package.as_ref().map(|_| ECOSYSTEM_CLI.to_owned()),
                package,
            }),
        )
        .await
        .gql()?;
        Ok(publishers.0.into_iter().map(Self).collect())
    }

    pub async fn load(ctx: &Context<'_>, id: Uuid) -> Result<Self> {
        Self::list(ctx, None)
            .await?
            .into_iter()
            .find(|p| p.0.id == id)
            .ok_or_else(|| async_graphql::Error::new(format!("trusted publisher {id} not found")))
    }
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryAuditEntry", complex)]
pub struct AuditEntry {
    #[graphql(skip)]
    pub row_id: i64,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub actor_kind: String,
    pub action: String,
    pub ecosystem: Option<String>,
    #[graphql(skip)]
    pub package: Option<String>,
    pub version: Option<String>,
    pub detail: Json<serde_json::Value>,
}

#[ComplexObject]
impl AuditEntry {
    async fn id(&self) -> ID {
        IDS.encode("auditEntry", self.row_id)
    }

    async fn package_name(&self) -> Option<&str> {
        self.package.as_deref()
    }
}

impl From<registry_api::AuditEntry> for AuditEntry {
    fn from(entry: registry_api::AuditEntry) -> Self {
        Self {
            row_id: entry.id,
            at: entry.at,
            actor: entry.actor,
            actor_kind: entry.actor_kind,
            action: entry.action,
            ecosystem: entry.ecosystem,
            package: entry.package,
            version: entry.version,
            detail: Json(entry.detail),
        }
    }
}

impl AuditEntry {
    pub async fn list(ctx: &Context<'_>, package: Option<String>, limit: i64) -> Result<Vec<Self>> {
        let entries = crate::core::audit::list(
            State(state(ctx).clone()),
            principal(ctx)?,
            Query(crate::core::audit::AuditQuery {
                ecosystem: package.as_ref().map(|_| ECOSYSTEM_CLI.to_owned()),
                package,
                limit,
            }),
        )
        .await
        .gql()?;
        Ok(entries.0.into_iter().map(Into::into).collect())
    }
}

#[derive(SimpleObject)]
#[graphql(name = "RegistrySigningKey", complex)]
pub struct SigningKey {
    pub key_id: String,
    pub public_key_pem: String,
}

#[ComplexObject]
impl SigningKey {
    async fn id(&self) -> ID {
        IDS.encode("signingKey", &self.key_id)
    }
}

impl SigningKey {
    pub async fn list(ctx: &Context<'_>) -> Vec<Self> {
        crate::core::signing::keys(State(state(ctx).clone()))
            .await
            .0
            .into_iter()
            .map(|key| Self {
                key_id: key.key_id,
                public_key_pem: key.public_key_pem,
            })
            .collect()
    }
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryWhoAmI")]
pub struct WhoAmI {
    pub subject: String,
    pub admin: bool,
    pub groups: Vec<String>,
}
