use std::{collections::BTreeMap, path::PathBuf};

use anyhow::Context;
use registry_api::{PackageAuth, Target};
use serde::{Deserialize, Serialize};

pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub packages: BTreeMap<String, InstalledPackage>,
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

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn package_dir(&self, name: &str) -> PathBuf {
        self.root.join("packages").join(name)
    }

    pub fn current_binary(&self, name: &str, package: &InstalledPackage) -> PathBuf {
        self.package_dir(name).join("current").join(&package.binary)
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

    #[test]
    fn resolves_names_and_aliases() {
        let mut installed = Installed::default();
        installed.packages.insert(
            "ff".to_owned(),
            InstalledPackage {
                version: "1.0.0".to_owned(),
                target: Target::LinuxAmd64,
                sha256: String::new(),
                binary: "ffctl".to_owned(),
                aliases: vec!["ffctl".to_owned()],
                auth: None,
            },
        );

        assert_eq!(installed.resolve("ff").unwrap().0, "ff");
        assert_eq!(installed.resolve("ffctl").unwrap().0, "ff");
        assert!(installed.resolve("ai").is_none());
    }
}
