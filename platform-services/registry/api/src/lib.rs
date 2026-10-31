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
        Ok(())
    }
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
