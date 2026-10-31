use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use registry_api::{
    Completions, DEFAULT_SCOPES, PackageAuth, Permissions, Target, validate_package_name,
};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "ortzi.yaml";
pub const DEFAULT_ISSUER_BASE: &str = "https://idm.anurag.sh/oauth2/openid";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: PackageSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completions: Option<Completions>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Cargo,
    Prebuilt,
    Wasm,
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
    pub prebuilt: BTreeMap<Target, Prebuilt>,
    #[serde(default, skip_serializing_if = "WasmBuild::is_default")]
    pub wasm: WasmBuild,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            kind: Kind::default(),
            targets: all_targets(),
            cargo: CargoBuild::default(),
            prebuilt: BTreeMap::new(),
            wasm: WasmBuild::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Prebuilt {
    Path(String),
    Remote {
        url: String,
        sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extract: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmBuild {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
}

impl WasmBuild {
    fn is_default(&self) -> bool {
        self == &Self::default()
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
        let mut manifest: Manifest = serde_yaml::from_str(&contents)
            .with_context(|| format!("parsing {}", file.display()))?;

        validate_package_name(&manifest.package.name).map_err(anyhow::Error::msg)?;
        for alias in &manifest.package.aliases {
            validate_package_name(alias).map_err(anyhow::Error::msg)?;
        }
        if let Some(auth) = &manifest.auth {
            auth.resolve().validate().map_err(anyhow::Error::msg)?;
        }
        if let Some(permissions) = &manifest.permissions {
            permissions.validate().map_err(anyhow::Error::msg)?;
        }
        if manifest.build.kind == Kind::Wasm {
            if manifest.build.targets != Target::NATIVE && manifest.build.targets != [Target::Wasm]
            {
                bail!("wasm packages only build the wasm target; drop build.targets");
            }
            manifest.build.targets = vec![Target::Wasm];
        } else if manifest.build.targets.contains(&Target::Wasm) {
            bail!("the wasm target needs build.kind: wasm");
        } else if manifest.permissions.is_some() {
            bail!("permissions only apply to wasm packages; native packages are not sandboxed");
        }
        if manifest.build.targets.is_empty() {
            bail!("build.targets is empty");
        }
        if manifest.build.kind == Kind::Prebuilt {
            for target in &manifest.build.targets {
                match manifest.build.prebuilt.get(target) {
                    None => bail!("build.prebuilt has no path for {target}"),
                    Some(Prebuilt::Remote { sha256, .. }) if !is_sha256(sha256) => {
                        bail!("build.prebuilt.{target}.sha256 must be 64 lowercase hex characters")
                    }
                    Some(_) => {}
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
            (None, Kind::Prebuilt | Kind::Wasm) => {
                bail!("package.version is required for prebuilt and wasm packages")
            }
        };
        Ok(
            semver::Version::parse(version.strip_prefix('v').unwrap_or(&version))
                .with_context(|| format!("invalid version {version:?}"))?
                .to_string(),
        )
    }

    pub async fn build(&self, target: Target, version: &str) -> anyhow::Result<PathBuf> {
        match self.manifest.build.kind {
            Kind::Prebuilt => match &self.manifest.build.prebuilt[&target] {
                Prebuilt::Path(pattern) => {
                    let path = self.dir.join(pattern.replace("${version}", version));
                    if !path.is_file() {
                        bail!(
                            "prebuilt {target} artifact {} does not exist",
                            path.display()
                        );
                    }
                    Ok(path)
                }
                Prebuilt::Remote {
                    url,
                    sha256,
                    extract,
                } => {
                    self.fetch(
                        target,
                        &url.replace("${version}", version),
                        sha256,
                        extract.as_deref(),
                        version,
                    )
                    .await
                }
            },
            Kind::Cargo => self.cargo_build(target),
            Kind::Wasm => self.wasm_build(),
        }
    }

    async fn fetch(
        &self,
        target: Target,
        url: &str,
        sha256: &str,
        extract: Option<&str>,
        version: &str,
    ) -> anyhow::Result<PathBuf> {
        use futures::TryStreamExt;
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncWriteExt;

        let dist = self.dir.join("dist");
        std::fs::create_dir_all(&dist)?;
        let download = dist.join(format!(".download-{target}"));
        let out = dist.join(format!("{}-{version}-{target}", self.binary()));

        eprintln!("downloading {target} from {url}");
        let response = reqwest::get(url)
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("downloading {url}"))?;
        let mut file = tokio::fs::File::create(&download).await?;
        let mut hasher = Sha256::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.try_next().await? {
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);

        let got = hex::encode(hasher.finalize());
        if got != sha256 {
            let _ = std::fs::remove_file(&download);
            bail!("upstream sha256 mismatch for {target}: expected {sha256}, got {got}");
        }

        match extract {
            None => std::fs::rename(&download, &out)?,
            Some(member) => {
                let unpack = dist.join(format!(".unpack-{target}"));
                let _ = std::fs::remove_dir_all(&unpack);
                std::fs::create_dir_all(&unpack)?;
                let status = Command::new("tar")
                    .arg("-xzf")
                    .arg(&download)
                    .arg("-C")
                    .arg(&unpack)
                    .arg(member)
                    .status()
                    .context("running tar")?;
                if !status.success() {
                    bail!("extracting {member} from {url} failed ({status})");
                }
                std::fs::rename(unpack.join(member), &out)
                    .with_context(|| format!("{url} has no {member}"))?;
                let _ = std::fs::remove_dir_all(&unpack);
                let _ = std::fs::remove_file(&download);
            }
        }

        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))?;
        Ok(out)
    }

    fn wasm_build(&self) -> anyhow::Result<PathBuf> {
        let entry = self
            .manifest
            .build
            .wasm
            .entry
            .as_deref()
            .unwrap_or("src/main.ts");
        let out = self
            .dir
            .join("dist")
            .join(format!("{}.wasm", self.binary()));

        eprintln!("building {} for wasm", self.name());
        let status = Command::new("pnpm")
            .args(["install", "--frozen-lockfile"])
            .current_dir(&self.dir)
            .status()
            .context("running pnpm; install it with `npm i -g pnpm`")?;
        if !status.success() {
            bail!("pnpm install failed ({status})");
        }

        let status = Command::new("pnpm")
            .args(["exec", "ortzi-wasm-build", "--entry", entry, "--out"])
            .arg(&out)
            .current_dir(&self.dir)
            .status()
            .context("running ortzi-wasm-build; add @ortzi/host as a dependency")?;
        if !status.success() {
            bail!("wasm build failed ({status})");
        }
        if !out.is_file() {
            bail!("expected build output at {}", out.display());
        }
        Ok(out)
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
            Prebuilt::Path("dist/tool-${version}-linux".to_owned())
        );
    }

    #[test]
    fn remote_prebuilt_manifest() {
        let manifest: Manifest = serde_yaml::from_str(&format!(
            r#"
package:
  name: bao
  version: 2.7.2
build:
  kind: prebuilt
  targets: [linux-amd64]
  prebuilt:
    linux-amd64:
      url: https://example.test/openbao_${{version}}_linux_amd64.tar.gz
      sha256: {}
      extract: bao
"#,
            "ab".repeat(32)
        ))
        .unwrap();
        assert_eq!(
            manifest.build.prebuilt[&Target::LinuxAmd64],
            Prebuilt::Remote {
                url: "https://example.test/openbao_${version}_linux_amd64.tar.gz".to_owned(),
                sha256: "ab".repeat(32),
                extract: Some("bao".to_owned()),
            }
        );
    }

    #[test]
    fn sha256_must_be_hex() {
        assert!(is_sha256(&"0f".repeat(32)));
        assert!(!is_sha256(&"0F".repeat(32)));
        assert!(!is_sha256("abc"));
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
    fn wasm_manifest() {
        let manifest: Manifest = serde_yaml::from_str(
            r#"
package:
  name: example
  version: 0.1.0
permissions:
  exec: [git]
  fs: [.]
build:
  kind: wasm
"#,
        )
        .unwrap();
        assert_eq!(manifest.build.kind, Kind::Wasm);
        assert_eq!(manifest.permissions.unwrap().exec, ["git"]);
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
