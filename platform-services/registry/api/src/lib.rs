#[cfg(feature = "oci")]
pub mod oci;
pub mod signing;

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn revision(version: &semver::Version) -> Option<u64> {
    version.pre.as_str().parse().ok()
}

pub fn is_release(version: &semver::Version) -> bool {
    version.pre.is_empty() || revision(version).is_some()
}

pub fn version_key(version: &semver::Version) -> impl Ord + use<> {
    let revision = revision(version);
    (
        (version.major, version.minor, version.patch),
        revision.is_some() || version.pre.is_empty(),
        if revision.is_some() {
            semver::Prerelease::EMPTY
        } else {
            version.pre.clone()
        },
        revision.unwrap_or(0),
        version.build.clone(),
    )
}

pub fn upstream_version(version: &str) -> &str {
    match version.rsplit_once('-') {
        Some((base, revision))
            if !revision.is_empty() && revision.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => version,
    }
}

pub const ECOSYSTEM_CLI: &str = "cli";

pub const SHA256_HEADER: &str = "x-ortzi-sha256";

pub const TOKEN_PREFIX: &str = "ortzi_";

pub const RESERVED_NAMES: &[&str] = &[
    "install",
    "uninstall",
    "update",
    "list",
    "search",
    "info",
    "login",
    "logout",
    "whoami",
    "publish",
    "init",
    "registry",
    "help",
    "version",
    "self",
    "self-update",
    "bin-dir",
];

pub fn validate_package_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let valid = name.len() <= 39
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');

    if !valid {
        return Err(format!(
            "invalid package name {name:?}: must match ^[a-z][a-z0-9-]{{0,38}}$"
        ));
    }

    if RESERVED_NAMES.contains(&name) {
        return Err(format!("{name:?} is reserved by ortzi"));
    }

    Ok(())
}

pub fn validate_binary_name(binary: &str) -> Result<(), String> {
    let valid = !binary.is_empty()
        && binary.len() <= 64
        && !binary.starts_with('.')
        && binary
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(format!("invalid binary name {binary:?}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Completions {
    Clap,
    Cobra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kube: Vec<KubeVerb>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http: Vec<String>,
    /// Loopback ports the package may serve HTTP on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listen: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KubeVerb {
    Get,
    Create,
    Update,
    Patch,
    Delete,
}

impl KubeVerb {
    pub fn for_method(method: &str) -> Option<Self> {
        match method {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Create),
            "PUT" => Some(Self::Update),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Create => "create",
            Self::Update => "update",
            Self::Patch => "patch",
            Self::Delete => "delete",
        }
    }
}

impl Permissions {
    pub fn validate(&self) -> Result<(), String> {
        for program in &self.exec {
            validate_binary_name(program).map_err(|_| {
                format!("permissions.exec: {program:?} must be a bare program name")
            })?;
        }
        for name in &self.env {
            if !is_env_name(name) {
                return Err(format!("permissions.env: invalid variable name {name:?}"));
            }
        }
        for path in &self.fs {
            let valid =
                path == "." || path == "~" || path.starts_with("~/") || path.starts_with('/');
            if !valid || path.split('/').any(|part| part == "..") {
                return Err(format!(
                    "permissions.fs: {path:?} must be \".\", \"~\", \"~/...\" or absolute, without \"..\""
                ));
            }
        }
        for host in &self.http {
            if !is_hostname(host) {
                return Err(format!(
                    "permissions.http: {host:?} must be a bare hostname"
                ));
            }
        }
        if self.listen.contains(&0) {
            return Err("permissions.listen: ports must be 1-65535".to_owned());
        }
        Ok(())
    }
}

fn is_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "darwin-arm64")]
    DarwinArm64,
    #[serde(rename = "linux-amd64")]
    LinuxAmd64,
    #[serde(rename = "wasm")]
    Wasm,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::DarwinArm64, Target::LinuxAmd64, Target::Wasm];
    pub const NATIVE: [Target; 2] = [Target::DarwinArm64, Target::LinuxAmd64];

    pub fn as_str(self) -> &'static str {
        match self {
            Target::DarwinArm64 => "darwin-arm64",
            Target::LinuxAmd64 => "linux-amd64",
            Target::Wasm => "wasm",
        }
    }

    pub fn rust_triple(self) -> &'static str {
        match self {
            Target::DarwinArm64 => "aarch64-apple-darwin",
            Target::LinuxAmd64 => "x86_64-unknown-linux-gnu",
            Target::Wasm => "wasm32-wasip2",
        }
    }

    pub fn runs_on(self, host: Target) -> bool {
        self == host || self == Target::Wasm
    }

    pub fn platform(self) -> (&'static str, &'static str) {
        match self {
            Target::DarwinArm64 => ("darwin", "arm64"),
            Target::LinuxAmd64 => ("linux", "amd64"),
            Target::Wasm => ("wasip2", "wasm"),
        }
    }

    pub fn file_name(self, binary: &str) -> String {
        match self {
            Target::Wasm => format!("{binary}.wasm"),
            _ => binary.to_owned(),
        }
    }

    pub fn media_type(self) -> &'static str {
        match self {
            Target::Wasm => WASM_MEDIA_TYPE,
            _ => BINARY_MEDIA_TYPE,
        }
    }

    pub fn host() -> Option<Target> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => Some(Target::DarwinArm64),
            ("linux", "x86_64") => Some(Target::LinuxAmd64),
            _ => None,
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Target {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Target::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown target {s:?} (expected one of: {})",
                    Target::ALL.map(Target::as_str).join(", ")
                )
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoAmI {
    pub subject: String,
    pub kind: String,
    pub admin: bool,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub ecosystem: String,
    pub package: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateToken {
    pub name: String,
    pub scopes: Vec<Scope>,
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedToken {
    pub id: Uuid,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<Scope>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPublisher {
    pub id: Uuid,
    pub ecosystem: String,
    pub package: String,
    pub provider: String,
    pub issuer: String,
    pub repository: String,
    pub repository_id: Option<String>,
    pub ref_pattern: Option<String>,
    pub workflow: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTrustedPublisher {
    #[serde(default = "default_ecosystem")]
    pub ecosystem: String,
    pub package: String,
    pub repository: String,
    #[serde(default)]
    pub ref_pattern: Option<String>,
    #[serde(default)]
    pub workflow: Option<String>,
}

fn default_ecosystem() -> String {
    ECOSYSTEM_CLI.to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub actor_kind: String,
    pub action: String,
    pub ecosystem: Option<String>,
    pub package: Option<String>,
    pub version: Option<String>,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub description: String,
    pub binary_name: String,
    pub aliases: Vec<String>,
    pub latest: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDetail {
    #[serde(flatten)]
    pub package: Package,
    pub versions: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionSummary {
    pub version: String,
    pub targets: Vec<Target>,
    pub published_at: Option<DateTime<Utc>>,
    pub yanked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePackage {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub binary_name: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdatePackage {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub binary_name: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageAuth {
    pub issuer: String,
    pub client_id: String,
    #[serde(default = "default_scopes")]
    pub scopes: String,
    pub token_env: String,
}

pub const DEFAULT_SCOPES: &str = "openid email profile groups";

fn default_scopes() -> String {
    DEFAULT_SCOPES.to_owned()
}

impl PackageAuth {
    pub fn validate(&self) -> Result<(), String> {
        let env_ok = self
            .token_env
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            && self
                .token_env
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !env_ok {
            return Err(format!("invalid auth.token_env {:?}", self.token_env));
        }
        if !self.issuer.starts_with("https://") {
            return Err(format!("auth.issuer must be https, got {:?}", self.issuer));
        }
        if self.client_id.is_empty() {
            return Err("auth.client_id is required".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVersion {
    pub version: String,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub binary_name: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub auth: Option<PackageAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub package: String,
    pub version: String,
    pub status: String,
    pub targets: Vec<Target>,
    pub description: String,
    pub binary_name: String,
    pub aliases: Vec<String>,
    #[serde(default)]
    pub auth: Option<PackageAuth>,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub signatures: Option<signing::Signatures>,
    pub published_at: Option<DateTime<Utc>>,
    pub published_by: Option<String>,
    pub source: Option<Source>,
    pub yanked: Option<Yanked>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub target: Target,
    pub sha256: String,
    pub size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestRequest {
    pub target: Target,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactUrl {
    pub url: String,
    pub expires_at: DateTime<Utc>,
    pub sha256: String,
    pub size: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub repository: Option<String>,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub sha: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Yanked {
    pub at: DateTime<Utc>,
    pub by: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Yank {
    #[serde(default)]
    pub reason: Option<String>,
}

pub const BINARY_ARTIFACT: &str = "application/vnd.ortzi.binary.v1";
pub const PACKAGE_ARTIFACT: &str = "application/vnd.ortzi.package.v1";
pub const SIGNATURES_ARTIFACT: &str = "application/vnd.ortzi.signatures.v1";
pub const SIGNATURES_MEDIA_TYPE: &str = "application/vnd.ortzi.signatures.v1+json";
pub const CATALOG_ARTIFACT: &str = "application/vnd.ortzi.catalog.v1";
pub const CATALOG_MEDIA_TYPE: &str = "application/vnd.ortzi.catalog.v1+json";
pub const BINARY_MEDIA_TYPE: &str = "application/octet-stream";
pub const WASM_MEDIA_TYPE: &str = "application/wasm";
pub const CATALOG_REPOSITORY: &str = "ortzi-catalog/catalog";
pub const CATALOG_PROJECT: &str = "ortzi-catalog";
pub const PACKAGES_PROJECT: &str = "ortzi-packages";
pub const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;

pub fn package_repository(project: &str, name: &str) -> String {
    format!("{project}/{name}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogRef {
    pub seq: i64,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterArtifact {
    pub manifest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub seq: i64,
    pub generated_at: DateTime<Utc>,
    pub packages_project: String,
    pub packages: std::collections::BTreeMap<String, CatalogPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogPackage {
    pub description: String,
    pub binary_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub archived: bool,
    pub versions: std::collections::BTreeMap<String, CatalogVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogVersion {
    pub targets: Vec<Target>,
    pub index: String,
    pub signatures: String,
    pub description: String,
    pub binary_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub auth: Option<PackageAuth>,
    pub published_at: DateTime<Utc>,
    #[serde(default)]
    pub published_by: Option<String>,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub yanked: Option<Yanked>,
}

impl Catalog {
    pub fn repository(&self, name: &str) -> String {
        package_repository(&self.packages_project, name)
    }

    pub fn version(&self, name: &str, version: &str) -> Option<(&str, &CatalogVersion)> {
        let wanted = semver::Version::parse(version.strip_prefix('v').unwrap_or(version))
            .map(|v| v.to_string())
            .unwrap_or_else(|_| version.to_owned());
        self.packages
            .get(name)?
            .versions
            .get_key_value(&wanted)
            .map(|(v, entry)| (v.as_str(), entry))
    }

    pub fn latest(
        &self,
        name: &str,
        target: Option<Target>,
        prerelease: bool,
    ) -> Option<(&str, &CatalogVersion)> {
        self.packages
            .get(name)?
            .versions
            .iter()
            .filter(|(_, v)| v.yanked.is_none())
            .filter(|(_, v)| target.is_none_or(|t| v.targets.iter().any(|v| v.runs_on(t))))
            .filter_map(|(version, v)| Some((semver::Version::parse(version).ok()?, version, v)))
            .filter(|(semver, _, _)| prerelease || is_release(semver))
            .max_by_key(|(semver, _, _)| version_key(semver))
            .map(|(_, version, v)| (version.as_str(), v))
    }
}

impl CatalogVersion {
    pub fn to_version(
        &self,
        package: &str,
        version: &str,
        signatures: signing::Signatures,
    ) -> Version {
        Version {
            package: package.to_owned(),
            version: version.to_owned(),
            status: "published".to_owned(),
            targets: self.targets.clone(),
            description: self.description.clone(),
            binary_name: self.binary_name.clone(),
            aliases: self.aliases.clone(),
            auth: self.auth.clone(),
            artifacts: Vec::new(),
            signatures: Some(signatures),
            published_at: Some(self.published_at),
            published_by: self.published_by.clone(),
            source: self.source.clone(),
            yanked: self.yanked.clone(),
        }
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    fn entry(targets: &[Target], yanked: bool) -> CatalogVersion {
        CatalogVersion {
            targets: targets.to_vec(),
            index: String::new(),
            signatures: String::new(),
            description: String::new(),
            binary_name: "ff".to_owned(),
            aliases: Vec::new(),
            auth: None,
            published_at: Utc::now(),
            published_by: None,
            source: None,
            yanked: yanked.then(|| Yanked {
                at: Utc::now(),
                by: "someone".to_owned(),
                reason: None,
            }),
        }
    }

    #[test]
    fn latest_matches_the_registry_rules() {
        let both = [Target::DarwinArm64, Target::LinuxAmd64];
        let versions = [
            ("0.9.0", entry(&both, false)),
            ("0.10.0", entry(&[Target::LinuxAmd64], false)),
            ("1.0.0", entry(&both, true)),
            ("1.1.0-rc.1", entry(&both, false)),
            ("3.0.0-2", entry(&[Target::Wasm], false)),
        ];
        let mut catalog = Catalog {
            seq: 1,
            generated_at: Utc::now(),
            packages_project: "ortzi-packages".to_owned(),
            packages: Default::default(),
        };
        catalog.packages.insert(
            "ff".to_owned(),
            CatalogPackage {
                description: String::new(),
                binary_name: "ff".to_owned(),
                aliases: Vec::new(),
                archived: false,
                versions: versions
                    .into_iter()
                    .map(|(v, e)| (v.to_owned(), e))
                    .collect(),
            },
        );

        let pick = |target, pre| catalog.latest("ff", target, pre).map(|(v, _)| v);
        assert_eq!(pick(None, false), Some("3.0.0-2"));
        assert_eq!(pick(Some(Target::DarwinArm64), false), Some("3.0.0-2"));
        catalog
            .packages
            .get_mut("ff")
            .unwrap()
            .versions
            .remove("3.0.0-2");
        let pick = |target, pre| catalog.latest("ff", target, pre).map(|(v, _)| v);
        assert_eq!(pick(None, false), Some("0.10.0"));
        assert_eq!(pick(Some(Target::DarwinArm64), false), Some("0.9.0"));
        assert_eq!(pick(None, true), Some("1.1.0-rc.1"));
        assert_eq!(
            catalog.version("ff", "v0.9.0").map(|(v, _)| v),
            Some("0.9.0")
        );
        assert_eq!(catalog.repository("ff"), "ortzi-packages/ff");
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    fn key(version: &str) -> impl Ord + use<> {
        version_key(&semver::Version::parse(version).unwrap())
    }

    #[test]
    fn revisions_sort_above_their_release() {
        assert!(key("2.7.2-rc.1") < key("2.7.2"));
        assert!(key("2.7.2") < key("2.7.2-1"));
        assert!(key("2.7.2-2") < key("2.7.2-10"));
        assert!(key("2.7.2-10") < key("2.7.3-rc.1"));
    }

    #[test]
    fn revisions_are_releases_and_expand_to_the_upstream_version() {
        assert!(is_release(&semver::Version::parse("2.7.2-2").unwrap()));
        assert!(!is_release(&semver::Version::parse("2.7.2-rc.1").unwrap()));
        assert_eq!(upstream_version("2.7.2-2"), "2.7.2");
        assert_eq!(upstream_version("2.7.2"), "2.7.2");
        assert_eq!(upstream_version("2.7.2-rc.1"), "2.7.2-rc.1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names() {
        assert!(validate_package_name("ff").is_ok());
        assert!(validate_package_name("janitor-2").is_ok());
        assert!(validate_package_name("Ff").is_err());
        assert!(validate_package_name("2ff").is_err());
        assert!(validate_package_name("ff_ctl").is_err());
        assert!(validate_package_name("").is_err());
        assert!(validate_package_name(&"a".repeat(40)).is_err());
        assert!(validate_package_name("install").is_err());
    }

    #[test]
    fn binary_names_cannot_escape_their_directory() {
        assert!(validate_binary_name("ffctl").is_ok());
        assert!(validate_binary_name("home-gateway_cli.v2").is_ok());
        for bad in ["", "../x", "a/b", "/etc/passwd", ".hidden", "..", "a\\b"] {
            assert!(validate_binary_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn targets_round_trip() {
        for target in Target::ALL {
            assert_eq!(target.as_str().parse::<Target>().unwrap(), target);
            assert_eq!(
                serde_json::to_string(&target).unwrap(),
                format!("\"{target}\"")
            );
        }
        assert!("windows-amd64".parse::<Target>().is_err());
        assert!(Target::Wasm.runs_on(Target::DarwinArm64));
        assert!(!Target::LinuxAmd64.runs_on(Target::DarwinArm64));
    }

    #[test]
    fn permission_validation() {
        let ok = Permissions {
            exec: vec!["kubectl".to_owned(), "ssh".to_owned()],
            env: vec!["KUBECONFIG".to_owned()],
            fs: vec![".".to_owned(), "~/.kube".to_owned(), "/tmp".to_owned()],
            kube: vec![KubeVerb::Get, KubeVerb::Patch],
            http: vec!["grafana.ortzi.cloud".to_owned(), "localhost".to_owned()],
            listen: vec![4280],
        };
        assert!(ok.validate().is_ok());
        for bad in [
            Permissions {
                exec: vec!["/bin/sh".to_owned()],
                ..Default::default()
            },
            Permissions {
                env: vec!["lower".to_owned()],
                ..Default::default()
            },
            Permissions {
                fs: vec!["relative".to_owned()],
                ..Default::default()
            },
            Permissions {
                fs: vec!["~/../etc".to_owned()],
                ..Default::default()
            },
            Permissions {
                http: vec!["https://grafana.ortzi.cloud".to_owned()],
                ..Default::default()
            },
            Permissions {
                http: vec!["grafana.ortzi.cloud:443".to_owned()],
                ..Default::default()
            },
            Permissions {
                listen: vec![0],
                ..Default::default()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn auth_validation() {
        let auth = PackageAuth {
            issuer: "https://idm.example.test/oauth2/openid/ff".to_owned(),
            client_id: "ff".to_owned(),
            scopes: DEFAULT_SCOPES.to_owned(),
            token_env: "FFCTL_TOKEN".to_owned(),
        };
        assert!(auth.validate().is_ok());
        assert!(
            PackageAuth {
                token_env: "ffctl-token".to_owned(),
                ..auth.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            PackageAuth {
                issuer: "http://idm".to_owned(),
                ..auth.clone()
            }
            .validate()
            .is_err()
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarborAction {
    Pull,
    Push,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarborAccess {
    pub project: String,
    pub actions: Vec<HarborAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborTokenRequest {
    pub access: Vec<HarborAccess>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarborToken {
    pub registry: String,
    pub username: String,
    pub password: String,
    pub expires_at: DateTime<Utc>,
    pub access: Vec<HarborAccess>,
}

pub const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.config+json";
