use chrono::{DateTime, Utc};
use registry_api::signing::Signatures;
use registry_api::{Artifact, PackageAuth, Source, Target, Version, VersionSummary, Yanked};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

use crate::error::{ApiError, Result};

pub const DRAFT: &str = "draft";
pub const PUBLISHED: &str = "published";

#[derive(Debug, Clone)]
pub struct PackageRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub binary_name: String,
    pub aliases: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct VersionRow {
    pub id: Uuid,
    pub package_id: Uuid,
    pub version: String,
    pub status: String,
    pub targets: Vec<String>,
    pub description: String,
    pub binary_name: String,
    pub aliases: Vec<String>,
    pub auth: Option<Json<PackageAuth>>,
    pub signatures: Option<Json<Signatures>>,
    pub published_at: Option<DateTime<Utc>>,
    pub published_by: Option<String>,
    pub source_repository: Option<String>,
    pub source_ref: Option<String>,
    pub source_sha: Option<String>,
    pub source_run_id: Option<String>,
    pub yanked_at: Option<DateTime<Utc>>,
    pub yanked_by: Option<String>,
    pub yank_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArtifactRow {
    pub target: String,
    pub sha256: String,
    pub size: i64,
    pub object_key: String,
}

impl VersionRow {
    pub fn is_published(&self) -> bool {
        self.status == PUBLISHED
    }

    pub fn semver(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.version).ok()
    }

    pub fn targets(&self) -> Vec<Target> {
        self.targets.iter().filter_map(|t| t.parse().ok()).collect()
    }

    pub fn summary(&self) -> VersionSummary {
        VersionSummary {
            version: self.version.clone(),
            targets: self.targets(),
            published_at: self.published_at,
            yanked: self.yanked_at.is_some(),
        }
    }

    pub fn into_api(self, package: &str, artifacts: &[ArtifactRow]) -> Version {
        Version {
            package: package.to_owned(),
            targets: self.targets(),
            artifacts: artifacts
                .iter()
                .filter_map(|a| {
                    Some(Artifact {
                        target: a.target.parse().ok()?,
                        sha256: a.sha256.clone(),
                        size: a.size,
                    })
                })
                .collect(),
            source: self.source_repository.is_some().then(|| Source {
                repository: self.source_repository.clone(),
                git_ref: self.source_ref.clone(),
                sha: self.source_sha.clone(),
                run_id: self.source_run_id.clone(),
            }),
            yanked: self.yanked_at.map(|at| Yanked {
                at,
                by: self.yanked_by.clone().unwrap_or_default(),
                reason: self.yank_reason.clone(),
            }),
            version: self.version,
            status: self.status,
            description: self.description,
            binary_name: self.binary_name,
            aliases: self.aliases,
            auth: self.auth.map(|a| a.0),
            signatures: self.signatures.map(|s| s.0),
            published_at: self.published_at,
            published_by: self.published_by,
        }
    }
}

pub fn parse_version(version: &str) -> Result<semver::Version> {
    semver::Version::parse(version.strip_prefix('v').unwrap_or(version))
        .map_err(|e| ApiError::BadRequest(format!("invalid semver {version:?}: {e}")))
}

pub fn latest(
    versions: &[VersionRow],
    target: Option<Target>,
    prerelease: bool,
) -> Option<&VersionRow> {
    versions
        .iter()
        .filter(|v| v.is_published() && v.yanked_at.is_none())
        .filter(|v| target.is_none_or(|t| v.targets().iter().any(|v| v.runs_on(t))))
        .filter_map(|v| Some((v.semver()?, v)))
        .filter(|(semver, _)| prerelease || semver.pre.is_empty())
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, v)| v)
}

pub async fn find_package(pool: &PgPool, name: &str) -> Result<PackageRow> {
    sqlx::query_as!(
        PackageRow,
        "SELECT id, name, description, binary_name, aliases, created_at
         FROM cli_packages WHERE name = $1",
        name
    )
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("package {name} not found")))
}

pub async fn all_packages(pool: &PgPool, query: Option<&str>) -> Result<Vec<PackageRow>> {
    let pattern = query.map(|q| format!("%{}%", q.replace('%', "\\%").replace('_', "\\_")));
    Ok(sqlx::query_as!(
        PackageRow,
        "SELECT id, name, description, binary_name, aliases, created_at
         FROM cli_packages
         WHERE $1::text IS NULL OR name ILIKE $1 OR description ILIKE $1
         ORDER BY name",
        pattern
    )
    .fetch_all(pool)
    .await?)
}

pub async fn versions(pool: &PgPool, package_id: Uuid) -> Result<Vec<VersionRow>> {
    Ok(sqlx::query_as!(
        VersionRow,
        r#"SELECT id, package_id, version, status, targets, description, binary_name, aliases,
                auth as "auth: Json<PackageAuth>", signatures as "signatures: Json<Signatures>", published_at, published_by, source_repository, source_ref, source_sha,
                source_run_id, yanked_at, yanked_by, yank_reason
         FROM cli_versions WHERE package_id = $1 ORDER BY created_at DESC"#,
        package_id
    )
    .fetch_all(pool)
    .await?)
}

pub async fn published_versions(pool: &PgPool) -> Result<Vec<VersionRow>> {
    Ok(sqlx::query_as!(
        VersionRow,
        r#"SELECT id, package_id, version, status, targets, description, binary_name, aliases,
                auth as "auth: Json<PackageAuth>", signatures as "signatures: Json<Signatures>", published_at, published_by, source_repository, source_ref, source_sha,
                source_run_id, yanked_at, yanked_by, yank_reason
         FROM cli_versions WHERE status = 'published'"#
    )
    .fetch_all(pool)
    .await?)
}

pub async fn find_version(
    pool: &PgPool,
    package_id: Uuid,
    version: &str,
) -> Result<Option<VersionRow>> {
    Ok(sqlx::query_as!(
        VersionRow,
        r#"SELECT id, package_id, version, status, targets, description, binary_name, aliases,
                auth as "auth: Json<PackageAuth>", signatures as "signatures: Json<Signatures>", published_at, published_by, source_repository, source_ref, source_sha,
                source_run_id, yanked_at, yanked_by, yank_reason
         FROM cli_versions WHERE package_id = $1 AND version = $2"#,
        package_id,
        version
    )
    .fetch_optional(pool)
    .await?)
}

pub async fn get_version(pool: &PgPool, package: &PackageRow, version: &str) -> Result<VersionRow> {
    let version = parse_version(version)?.to_string();
    find_version(pool, package.id, &version)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("{}@{version} not found", package.name)))
}

pub async fn artifacts(pool: &PgPool, version_id: Uuid) -> Result<Vec<ArtifactRow>> {
    Ok(sqlx::query_as!(
        ArtifactRow,
        "SELECT target, sha256, size, object_key FROM cli_artifacts
         WHERE version_id = $1 ORDER BY target",
        version_id
    )
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(v: &str, published: bool, yanked: bool, targets: &[&str]) -> VersionRow {
        VersionRow {
            id: Uuid::nil(),
            package_id: Uuid::nil(),
            version: v.to_owned(),
            status: if published { PUBLISHED } else { DRAFT }.to_owned(),
            targets: targets.iter().map(|t| (*t).to_owned()).collect(),
            description: String::new(),
            binary_name: "ff".to_owned(),
            aliases: Vec::new(),
            auth: None,
            signatures: None,
            published_at: None,
            published_by: None,
            source_repository: None,
            source_ref: None,
            source_sha: None,
            source_run_id: None,
            yanked_at: yanked.then(Utc::now),
            yanked_by: None,
            yank_reason: None,
        }
    }

    #[test]
    fn latest_skips_drafts_yanked_and_prereleases() {
        let both = ["darwin-arm64", "linux-amd64"];
        let versions = vec![
            version("0.9.0", true, false, &both),
            version("0.10.0", true, false, &["linux-amd64"]),
            version("1.0.0", true, true, &both),
            version("1.1.0-rc.1", true, false, &both),
            version("2.0.0", false, false, &both),
        ];

        let pick = |target, pre| latest(&versions, target, pre).map(|v| v.version.as_str());

        assert_eq!(pick(None, false), Some("0.10.0"));
        assert_eq!(pick(Some(Target::DarwinArm64), false), Some("0.9.0"));
        assert_eq!(pick(None, true), Some("1.1.0-rc.1"));
    }

    #[test]
    fn versions_are_canonicalised() {
        assert_eq!(parse_version("v1.2.3").unwrap().to_string(), "1.2.3");
        assert!(parse_version("1.2").is_err());
    }
}
