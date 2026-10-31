use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use cli_auth::Credentials;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use registry_api::{Target, Version};

use crate::{
    client::{self, Client},
    commands::packages::{self, SELF},
    dispatch, package_auth,
    session::Settings,
    store::{Installed, InstalledPackage, Store},
    trust,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug)]
struct Finding {
    check: String,
    level: Level,
    detail: String,
}

impl Finding {
    fn new(check: &str, level: Level, detail: impl Into<String>) -> Self {
        Self {
            check: check.to_owned(),
            level,
            detail: detail.into(),
        }
    }
}

pub async fn run(settings: &Settings) -> anyhow::Result<()> {
    let store = Store::open()?;
    let installed = store.load()?;
    let target = Target::host();

    let mut findings = install_findings();
    findings.extend(store_findings(&store, &installed));

    let client = match settings.reader_without_login().await {
        Ok(Some(client)) => match client.get::<registry_api::WhoAmI>("/api/v1/whoami").await {
            Ok(me) => {
                findings.push(Finding::new(
                    "registry",
                    Level::Ok,
                    format!("{} as {}", settings.registry, me.subject),
                ));
                Some(client)
            }
            Err(e) => {
                findings.push(Finding::new(
                    "registry",
                    Level::Fail,
                    format!("{}: {e:#}", settings.registry),
                ));
                None
            }
        },
        Ok(None) => {
            findings.push(Finding::new(
                "registry",
                Level::Warn,
                "not logged in; run `ortzi login`",
            ));
            None
        }
        Err(e) => {
            findings.push(Finding::new(
                "registry",
                Level::Fail,
                format!("session refresh failed: {e:#}; run `ortzi login`"),
            ));
            None
        }
    };

    let mut latest = None;
    if let (Some(client), Some(target)) = (&client, target) {
        let names: Vec<&str> = installed
            .packages
            .iter()
            .filter(|(_, package)| !package.pinned)
            .map(|(name, _)| name.as_str())
            .chain((!installed.self_pinned).then_some(SELF))
            .collect();
        match packages::latest_versions(client, &names, target).await {
            Ok(versions) => latest = Some(versions),
            Err(e) => findings.push(Finding::new(
                "registry",
                Level::Warn,
                format!("could not check for updates: {e:#}"),
            )),
        }
        if let Some(versions) = &latest {
            findings.push(self_finding(versions, installed.self_pinned));
        }
    }

    for (name, package) in &installed.packages {
        let mut problems = package_findings(&store, name, package).await;
        if let Some(client) = &client {
            problems.extend(remote_findings(client, name, package, latest.as_ref()).await);
        }
        if problems.is_empty() {
            let pin = if package.pinned { " (pinned)" } else { "" };
            findings.push(Finding::new(
                name,
                Level::Ok,
                format!("{}{pin}", package.version),
            ));
        } else {
            findings.extend(problems);
        }
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_BORDERS_ONLY)
        .set_header(["check", "status", "detail"]);
    for finding in &findings {
        table.add_row([
            finding.check.as_str(),
            match finding.level {
                Level::Ok => "ok",
                Level::Warn => "warn",
                Level::Fail => "fail",
            },
            &finding.detail,
        ]);
    }
    println!("{table}");

    if findings.iter().any(|f| f.level == Level::Fail) {
        std::process::exit(1);
    }
    Ok(())
}

fn install_findings() -> Vec<Finding> {
    let mut findings = Vec::new();

    if let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize())
        && exe.components().any(|c| c.as_os_str() == ".cargo")
    {
        findings.push(Finding::new(
            "install",
            Level::Warn,
            format!(
                "{} is cargo-managed; run `just platform install`, then `cargo uninstall ortzi`",
                exe.display()
            ),
        ));
    }
    findings
}

fn store_findings(store: &Store, installed: &Installed) -> Vec<Finding> {
    let names = |dir: &Path| -> BTreeSet<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    };

    let mut findings = Vec::new();
    for orphan in names(&store.packages_dir())
        .into_iter()
        .filter(|n| !installed.packages.contains_key(n))
    {
        findings.push(Finding::new(
            "store",
            Level::Warn,
            format!(
                "{} is not in installed.json",
                store.package_dir(&orphan).display()
            ),
        ));
    }
    for orphan in names(&store.bin_dir())
        .into_iter()
        .filter(|n| !installed.packages.values().any(|p| &p.binary == n))
    {
        findings.push(Finding::new(
            "store",
            Level::Warn,
            format!(
                "{} is not owned by a package",
                store.bin_dir().join(orphan).display()
            ),
        ));
    }
    findings
}

async fn package_findings(store: &Store, name: &str, package: &InstalledPackage) -> Vec<Finding> {
    let reinstall = format!("run `ortzi install --force {name}`");
    let mut findings = Vec::new();

    let binary = store.current_binary(name, package);
    if !binary.exists() {
        findings.push(Finding::new(
            name,
            Level::Fail,
            format!("{} is missing; {reinstall}", binary.display()),
        ));
        return findings;
    }
    match client::sha256_file(&binary).await {
        Ok(sha) if sha == package.sha256 => {}
        Ok(_) => findings.push(Finding::new(
            name,
            Level::Fail,
            format!(
                "{} does not match its checksum; {reinstall}",
                binary.display()
            ),
        )),
        Err(e) => findings.push(Finding::new(name, Level::Fail, format!("{e:#}"))),
    }

    if !package.is_wasm() {
        let shim = store.bin_dir().join(&package.binary);
        if std::fs::read_link(&shim).ok() != Some(packages::shim_target(name, &package.binary)) {
            findings.push(Finding::new(
                name,
                Level::Fail,
                format!("{} is not linked to {name}; {reinstall}", shim.display()),
            ));
        }
    }

    if let Some(auth) = &package.auth
        && std::env::var_os(&auth.token_env).is_none()
    {
        let oauth = package_auth::oauth(name, auth);
        let usable = Credentials::load(oauth.app(), oauth.issuer(), oauth.client_id())
            .is_some_and(|c| c.is_fresh() || c.refresh_token.is_some());
        if !usable {
            findings.push(Finding::new(
                name,
                Level::Warn,
                format!("not logged in; run `ortzi login {name}`"),
            ));
        }
    }

    for program in package.permissions.iter().flat_map(|p| &p.exec) {
        if dispatch::which(program).is_none() {
            findings.push(Finding::new(
                name,
                Level::Warn,
                format!("needs {program}, which is not on PATH"),
            ));
        }
    }
    findings
}

fn self_finding(latest: &BTreeMap<String, String>, pinned: bool) -> Finding {
    let running = env!("ORTZI_VERSION");
    if pinned {
        return Finding::new(SELF, Level::Ok, format!("{running} (pinned)"));
    }
    match latest.get(SELF) {
        Some(version) if packages::is_newer(version, running) => Finding::new(
            SELF,
            Level::Warn,
            format!("{running} installed, {version} available; run `ortzi self-update`"),
        ),
        Some(_) => Finding::new(SELF, Level::Ok, running),
        None => Finding::new(SELF, Level::Warn, "no published build for this platform"),
    }
}

async fn remote_findings(
    client: &Client,
    name: &str,
    package: &InstalledPackage,
    latest: Option<&BTreeMap<String, String>>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    let path = format!("/cli/v1/packages/{name}/versions/{}", package.version);
    match client.find::<Version>(&path).await {
        Ok(Some(version)) => {
            match trust::verified_statement(name, &version) {
                Ok(statement) => {
                    if !statement
                        .artifacts
                        .iter()
                        .any(|a| a.target == package.target && a.sha256 == package.sha256)
                    {
                        findings.push(Finding::new(
                            name,
                            Level::Fail,
                            "installed checksum is not in the signed release",
                        ));
                    }
                }
                Err(e) => findings.push(Finding::new(name, Level::Fail, format!("{e:#}"))),
            }
            if let Some(yanked) = &version.yanked {
                findings.push(Finding::new(
                    name,
                    Level::Warn,
                    format!(
                        "{} was yanked: {}",
                        package.version,
                        yanked.reason.as_deref().unwrap_or("no reason given")
                    ),
                ));
            }
        }
        Ok(None) => findings.push(Finding::new(
            name,
            Level::Warn,
            format!("{} is no longer in the registry", package.version),
        )),
        Err(e) => findings.push(Finding::new(name, Level::Warn, format!("{e:#}"))),
    }

    if let Some(latest) = latest.filter(|_| !package.pinned) {
        match latest.get(name) {
            Some(version) if packages::is_newer(version, &package.version) => {
                findings.push(Finding::new(
                    name,
                    Level::Warn,
                    format!(
                        "{} installed, {version} available; run `ortzi update {name}`",
                        package.version
                    ),
                ));
            }
            Some(_) => {}
            None => findings.push(Finding::new(
                name,
                Level::Warn,
                "no published build for this platform",
            )),
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn package(binary: &str, sha256: String) -> InstalledPackage {
        InstalledPackage {
            version: "1.0.0".to_owned(),
            target: Target::LinuxAmd64,
            sha256,
            binary: binary.to_owned(),
            aliases: Vec::new(),
            auth: None,
            permissions: None,
            completions: None,
            env: Default::default(),
            pinned: false,
            previous: None,
        }
    }

    fn fixture(name: &str) -> (Store, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("ortzi-doctor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let version = root.join("packages/ff/1.0.0");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        symlink("1.0.0", root.join("packages/ff/current")).unwrap();
        std::fs::write(version.join("ffctl"), "binary").unwrap();
        (Store::at(root.clone()), root)
    }

    fn levels(findings: &[Finding]) -> Vec<Level> {
        findings.iter().map(|f| f.level).collect()
    }

    #[tokio::test]
    async fn healthy_package_has_no_findings() {
        let (store, root) = fixture("healthy");
        symlink(packages::shim_target("ff", "ffctl"), root.join("bin/ffctl")).unwrap();
        let sha = client::sha256_file(&root.join("packages/ff/1.0.0/ffctl"))
            .await
            .unwrap();
        let findings = package_findings(&store, "ff", &package("ffctl", sha)).await;
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[tokio::test]
    async fn reports_broken_installs() {
        let (store, root) = fixture("broken");
        let findings = package_findings(&store, "ff", &package("ffctl", "bad".to_owned())).await;
        assert_eq!(levels(&findings), [Level::Fail, Level::Fail]);

        std::fs::remove_file(root.join("packages/ff/1.0.0/ffctl")).unwrap();
        let findings = package_findings(&store, "ff", &package("ffctl", "bad".to_owned())).await;
        assert_eq!(levels(&findings), [Level::Fail]);
        assert!(findings[0].detail.contains("is missing"));
    }

    #[test]
    fn reports_orphans() {
        let (store, root) = fixture("orphans");
        std::fs::write(root.join("bin/stray"), "").unwrap();
        let findings = store_findings(&store, &Installed::default());
        assert_eq!(levels(&findings), [Level::Warn, Level::Warn]);
    }
}
