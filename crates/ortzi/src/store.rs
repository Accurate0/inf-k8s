use std::{collections::BTreeMap, path::PathBuf};

use anyhow::Context;
use registry_api::{Completions, PackageAuth, Permissions, Target};
use serde::{Deserialize, Serialize};

pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub packages: BTreeMap<String, InstalledPackage>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub self_pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub version: String,
    pub target: Target,
    pub sha256: String,
    pub binary: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<PackageAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completions: Option<Completions>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<InstalledPackage>>,
}

impl InstalledPackage {
    pub fn replacing(mut self, existing: &InstalledPackage) -> Self {
        self.pinned = existing.pinned;
        self.previous = if existing.version == self.version {
            existing.previous.clone()
        } else {
            Some(Box::new(InstalledPackage {
                previous: None,
                ..existing.clone()
            }))
        };
        self
    }

    pub fn rolled_back(&self) -> Option<Self> {
        let mut target = (**self.previous.as_ref()?).clone();
        target.pinned = self.pinned;
        target.previous = Some(Box::new(InstalledPackage {
            previous: None,
            ..self.clone()
        }));
        Some(target)
    }

    pub fn is_wasm(&self) -> bool {
        self.target == Target::Wasm
    }

    pub fn file_name(&self) -> String {
        file_name(&self.binary, self.target)
    }
}

pub fn file_name(binary: &str, target: Target) -> String {
    match target {
        Target::Wasm => format!("{binary}.wasm"),
        _ => binary.to_owned(),
    }
}

impl Installed {
    pub fn resolve(&self, command: &str) -> Option<(&String, &InstalledPackage)> {
        self.packages.get_key_value(command).or_else(|| {
            self.packages
                .iter()
                .find(|(_, p)| p.aliases.iter().any(|a| a == command))
        })
    }
}

impl Store {
    pub fn open() -> anyhow::Result<Self> {
        let root = match std::env::var_os("ORTZI_HOME") {
            Some(home) => PathBuf::from(home),
            None => match std::env::var_os("XDG_DATA_HOME") {
                Some(data) => PathBuf::from(data).join("ortzi"),
                None => dirs::home_dir()
                    .context("no home directory")?
                    .join(".local/share/ortzi"),
            },
        };
        Ok(Self { root })
    }

    #[cfg(test)]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn self_backup(&self) -> PathBuf {
        self.root.join("self").join("previous")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn packages_dir(&self) -> PathBuf {
        self.root.join("packages")
    }

    pub fn package_dir(&self, name: &str) -> PathBuf {
        self.packages_dir().join(name)
    }

    pub fn current_binary(&self, name: &str, package: &InstalledPackage) -> PathBuf {
        self.package_dir(name)
            .join("current")
            .join(package.file_name())
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("installed.json")
    }

    pub fn load(&self) -> anyhow::Result<Installed> {
        match std::fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", self.state_path().display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Installed::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, installed: &Installed) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let tmp = self.root.join("installed.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(installed)?)?;
        std::fs::rename(&tmp, self.state_path())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(version: &str) -> InstalledPackage {
        InstalledPackage {
            version: version.to_owned(),
            target: Target::LinuxAmd64,
            sha256: String::new(),
            binary: "ffctl".to_owned(),
            aliases: vec!["ffctl".to_owned()],
            auth: None,
            permissions: None,
            completions: None,
            pinned: false,
            previous: None,
        }
    }

    #[test]
    fn replacing_keeps_one_previous_version() {
        let first = package("1.0.0");
        let second = package("2.0.0").replacing(&first);
        let third = package("3.0.0").replacing(&second);
        let previous = third.previous.as_ref().unwrap();
        assert_eq!(previous.version, "2.0.0");
        assert!(previous.previous.is_none());

        let reinstalled = package("3.0.0").replacing(&third);
        assert_eq!(reinstalled.previous.unwrap().version, "2.0.0");
    }

    #[test]
    fn rollback_swaps_and_keeps_the_pin() {
        let mut current = package("2.0.0").replacing(&package("1.0.0"));
        current.pinned = true;
        let rolled = current.rolled_back().unwrap();
        assert_eq!(rolled.version, "1.0.0");
        assert!(rolled.pinned);
        assert_eq!(rolled.rolled_back().unwrap().version, "2.0.0");
        assert!(package("1.0.0").rolled_back().is_none());
    }

    #[test]
    fn resolves_names_and_aliases() {
        let mut installed = Installed::default();
        installed.packages.insert("ff".to_owned(), package("1.0.0"));

        assert_eq!(installed.resolve("ff").unwrap().0, "ff");
        assert_eq!(installed.resolve("ffctl").unwrap().0, "ff");
        assert!(installed.resolve("ai").is_none());
    }
}
