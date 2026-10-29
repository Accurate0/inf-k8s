use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use registry_api::{DEFAULT_SCOPES, PackageAuth, Target, validate_package_name};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "ortzi.yaml";
pub const DEFAULT_ISSUER_BASE: &str = "https://idm.anurag.sh/oauth2/openid";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: PackageSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSection>,
    #[serde(default)]
    pub build: Build,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSection {
    pub client_id: String,
    pub token_env: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
}

impl AuthSection {
    pub fn resolve(&self) -> PackageAuth {
        PackageAuth {
            issuer: self
                .issuer
                .clone()
                .unwrap_or_else(|| format!("{DEFAULT_ISSUER_BASE}/{}", self.client_id)),
            client_id: self.client_id.clone(),
            scopes: self
                .scopes
                .clone()
                .unwrap_or_else(|| DEFAULT_SCOPES.to_owned()),
            token_env: self.token_env.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Cargo,
    Prebuilt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    #[serde(default)]
    pub kind: Kind,
    #[serde(default = "all_targets")]
    pub targets: Vec<Target>,
    #[serde(default, skip_serializing_if = "CargoBuild::is_default")]
    pub cargo: CargoBuild,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prebuilt: BTreeMap<Target, String>,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            kind: Kind::default(),
            targets: all_targets(),
            cargo: CargoBuild::default(),
            prebuilt: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_default_features: bool,
}

impl CargoBuild {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn all_targets() -> Vec<Target> {
    Target::NATIVE.to_vec()
}

#[derive(Debug)]
pub struct Loaded {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Loaded {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file = if path.is_dir() {
            path.join(FILE_NAME)
        } else {
            path.to_owned()
        };
        let contents = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}; create one with `ortzi init`", file.display()))?;
        let manifest: Manifest = serde_yaml::from_str(&contents)
            .with_context(|| format!("parsing {}", file.display()))?;

        validate_package_name(&manifest.package.name).map_err(anyhow::Error::msg)?;
        for alias in &manifest.package.aliases {
            validate_package_name(alias).map_err(anyhow::Error::msg)?;
        }
        if let Some(auth) = &manifest.auth {
            auth.resolve().validate().map_err(anyhow::Error::msg)?;
        }
        if manifest.build.targets.is_empty() {
            bail!("build.targets is empty");
        }
        if manifest.build.kind == Kind::Prebuilt {
            for target in &manifest.build.targets {
                if !manifest.build.prebuilt.contains_key(target) {
                    bail!("build.prebuilt has no path for {target}");
                }
            }
        }

        let dir = file
            .parent()
            .map(Path::to_owned)
            .unwrap_or_default()
            .canonicalize()?;
        Ok(Self { manifest, dir })
    }

    pub fn name(&self) -> &str {
        &self.manifest.package.name
    }

    pub fn binary(&self) -> &str {
        self.manifest
            .package
            .binary
            .as_deref()
            .unwrap_or(&self.manifest.package.name)
    }

    fn cargo_manifest(&self) -> PathBuf {
        self.dir.join(
            self.manifest
                .build
                .cargo
                .manifest_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
        )
    }

    fn cargo_package(&self) -> &str {
        self.manifest
            .build
            .cargo
            .package
            .as_deref()
            .unwrap_or(self.binary())
    }

    fn cargo_bin(&self) -> &str {
        self.manifest
            .build
            .cargo
            .bin
            .as_deref()
            .unwrap_or(self.binary())
    }

    pub fn version(&self) -> anyhow::Result<String> {
        let version = match (&self.manifest.package.version, self.manifest.build.kind) {
            (Some(version), _) => version.clone(),
            (None, Kind::Cargo) => {
                let metadata = cargo_metadata(&self.cargo_manifest())?;
                let package = self.cargo_package();
                metadata
                    .packages
                    .into_iter()
                    .find(|p| p.name == package)
                    .with_context(|| format!("cargo package {package} not found"))?
                    .version
            }
            (None, Kind::Prebuilt) => bail!("package.version is required for prebuilt packages"),
        };
        Ok(
            semver::Version::parse(version.strip_prefix('v').unwrap_or(&version))
                .with_context(|| format!("invalid version {version:?}"))?
                .to_string(),
        )
    }

    pub fn build(&self, target: Target, version: &str) -> anyhow::Result<PathBuf> {
        match self.manifest.build.kind {
            Kind::Prebuilt => {
                let pattern = &self.manifest.build.prebuilt[&target];
                let path = self.dir.join(pattern.replace("${version}", version));
                if !path.is_file() {
                    bail!(
                        "prebuilt {target} artifact {} does not exist",
                        path.display()
                    );
                }
                Ok(path)
            }
            Kind::Cargo => self.cargo_build(target),
        }
    }

    fn cargo_build(&self, target: Target) -> anyhow::Result<PathBuf> {
        let manifest = self.cargo_manifest();
        let cargo = &self.manifest.build.cargo;
        let native = rustc_host()? == target.rust_triple();

        let mut command = Command::new("cargo");
        if native {
            command.args(["build", "--target", target.rust_triple()]);
        } else {
            command.args(["zigbuild", "--target", zig_triple(target)]);
        }
        command
            .args(["--release", "--locked", "--manifest-path"])
            .arg(&manifest)
            .args(["-p", self.cargo_package(), "--bin", self.cargo_bin()]);
        if !cargo.features.is_empty() {
            command.args(["--features", &cargo.features.join(",")]);
        }
        if cargo.no_default_features {
            command.arg("--no-default-features");
        }

        eprintln!("building {} for {target}", self.name());
        let status = command.status().with_context(|| {
            if native {
                "running cargo".to_owned()
            } else {
                "running cargo zigbuild; install it with `cargo install cargo-zigbuild`".to_owned()
            }
        })?;
        if !status.success() {
            bail!("build for {target} failed ({status})");
        }

        let target_dir = cargo_metadata(&manifest)?.target_directory;
        let path = target_dir
            .join(target.rust_triple())
            .join("release")
            .join(self.cargo_bin());
        if !path.is_file() {
            bail!("expected build output at {}", path.display());
        }
        Ok(path)
    }
}

fn rustc_host() -> anyhow::Result<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("running rustc -vV")?;
    if !output.status.success() {
        bail!(
            "rustc -vV failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .context("rustc -vV did not report a host triple")
}

fn zig_triple(target: Target) -> &'static str {
    match target {
        Target::LinuxAmd64 => "x86_64-unknown-linux-gnu.2.28",
        other => other.rust_triple(),
    }
}

#[derive(Deserialize)]
pub struct CargoMetadata {
    pub packages: Vec<CargoPackage>,
    pub target_directory: PathBuf,
}

#[derive(Deserialize)]
pub struct CargoPackage {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub targets: Vec<CargoTarget>,
}

#[derive(Deserialize)]
pub struct CargoTarget {
    pub name: String,
    pub kind: Vec<String>,
}

pub fn cargo_metadata(manifest: &Path) -> anyhow::Result<CargoMetadata> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed for {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_manifest_defaults() {
        let manifest: Manifest = serde_yaml::from_str("package:\n  name: ff\n").unwrap();
        assert_eq!(manifest.build.kind, Kind::Cargo);
        assert_eq!(manifest.build.targets, Target::NATIVE.to_vec());
        assert!(manifest.package.version.is_none());
    }

    #[test]
    fn prebuilt_manifest() {
        let manifest: Manifest = serde_yaml::from_str(
            r#"
package:
  name: tool
  version: 1.2.3
build:
  kind: prebuilt
  targets: [linux-amd64]
  prebuilt:
    linux-amd64: dist/tool-${version}-linux
"#,
        )
        .unwrap();
        assert_eq!(manifest.build.kind, Kind::Prebuilt);
        assert_eq!(
            manifest.build.prebuilt[&Target::LinuxAmd64],
            "dist/tool-${version}-linux"
        );
    }

    #[test]
    fn auth_defaults_the_issuer_from_the_client() {
        let manifest: Manifest = serde_yaml::from_str(
            "package:\n  name: ff\nauth:\n  client_id: feature-flags\n  token_env: FFCTL_TOKEN\n",
        )
        .unwrap();
        let auth = manifest.auth.unwrap().resolve();
        assert_eq!(
            auth.issuer,
            "https://idm.anurag.sh/oauth2/openid/feature-flags"
        );
        assert_eq!(auth.scopes, DEFAULT_SCOPES);
        assert!(auth.validate().is_ok());
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(serde_yaml::from_str::<Manifest>("package:\n  name: ff\n  bianry: x\n").is_err());
    }

    #[test]
    fn linux_builds_pin_glibc() {
        assert_eq!(
            zig_triple(Target::LinuxAmd64),
            "x86_64-unknown-linux-gnu.2.28"
        );
        assert_eq!(zig_triple(Target::DarwinArm64), "aarch64-apple-darwin");
    }
}
