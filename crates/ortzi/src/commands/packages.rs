use std::path::Path;

use anyhow::{Context, bail};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use registry_api::{
    Package, PackageDetail, Permissions, Target, Version, validate_binary_name,
    validate_package_name,
};

use crate::{
    client::{self, Client},
    session::Settings,
    store::{self, Installed, InstalledPackage, Store},
    trust, wasm,
};

fn host_target() -> anyhow::Result<Target> {
    Target::host().with_context(|| {
        format!(
            "{}-{} is not a supported target (supported: {})",
            std::env::consts::OS,
            std::env::consts::ARCH,
            Target::NATIVE.map(Target::as_str).join(", ")
        )
    })
}

pub const SELF: &str = "ortzi";

fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (spec, None),
    }
}

async fn resolve(
    client: &Client,
    name: &str,
    version: Option<&str>,
    target: Target,
) -> anyhow::Result<Version> {
    let found = match version {
        Some(version) => {
            client
                .find(&format!("/cli/v1/packages/{name}/versions/{version}"))
                .await?
        }
        None => {
            client
                .find(&format!("/cli/v1/packages/{name}/latest?target={target}"))
                .await?
        }
    };
    found.with_context(|| match version {
        Some(version) => format!("{name}@{version} not found"),
        None => format!("no published {name} for {target}; see `ortzi search`"),
    })
}

async fn install_version(
    client: &Client,
    store: &Store,
    name: &str,
    version: &Version,
    target: Target,
    force: bool,
) -> anyhow::Result<()> {
    validate_package_name(name).map_err(anyhow::Error::msg)?;
    let trust::Verified {
        artifact,
        permissions,
    } = trust::verified_artifact(name, version, target)?;
    let target = artifact.target;
    validate_binary_name(&version.binary_name).map_err(anyhow::Error::msg)?;
    if let Some(yanked) = &version.yanked {
        eprintln!(
            "warning: {name}@{} was yanked by {}: {}",
            version.version,
            yanked.by,
            yanked.reason.as_deref().unwrap_or("no reason given")
        );
    }

    let mut installed = store.load()?;
    check_conflicts(&installed, name, version)?;
    if !force
        && let Some(current) = installed.packages.get(name)
        && current.version == version.version
        && current.target == target
        && current.sha256 == artifact.sha256
        && store.current_binary(name, current).exists()
    {
        println!("{name}@{} is already installed", version.version);
        return Ok(());
    }

    std::fs::create_dir_all(store.tmp_dir())?;
    let download =
        store
            .tmp_dir()
            .join(format!("{name}-{}-{}", version.version, std::process::id()));
    eprintln!(
        "downloading {name}@{} ({target}, {})",
        version.version,
        client::human_size(artifact.size)
    );
    let sha = client
        .download(
            &format!(
                "/cli/v1/packages/{name}/versions/{}/artifacts/{target}",
                version.version
            ),
            &download,
        )
        .await?;
    if sha != artifact.sha256 {
        let _ = std::fs::remove_file(&download);
        bail!(
            "checksum mismatch for {name}@{}: expected {}, got {sha}",
            version.version,
            artifact.sha256
        );
    }
    set_executable(&download)?;

    let package_dir = store.package_dir(name);
    let version_dir = package_dir.join(&version.version);
    std::fs::create_dir_all(&version_dir)?;
    let installed_file = version_dir.join(store::file_name(&version.binary_name, target));
    std::fs::rename(&download, &installed_file)?;
    if target == Target::Wasm {
        eprintln!("compiling {name}@{}", version.version);
        wasm::prepare(&installed_file)?;
    }

    let current = package_dir.join("current");
    let staged = package_dir.join("current.tmp");
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(&version.version, &staged)?;
    std::fs::rename(&staged, &current)?;

    let previous = installed.packages.insert(
        name.to_owned(),
        InstalledPackage {
            version: version.version.clone(),
            target,
            sha256: artifact.sha256.clone(),
            binary: version.binary_name.clone(),
            aliases: version.aliases.clone(),
            auth: version.auth.clone(),
            permissions: permissions.clone(),
        },
    );
    store.save(&installed)?;
    if target == Target::Wasm {
        let _ = std::fs::remove_file(store.bin_dir().join(&version.binary_name));
    } else {
        link_shim(store, name, &version.binary_name)?;
    }

    if let Some(previous) = previous {
        if previous.binary != version.binary_name {
            let _ = std::fs::remove_file(store.bin_dir().join(&previous.binary));
        }
        if previous.version != version.version {
            let _ = std::fs::remove_dir_all(package_dir.join(&previous.version));
        }
        println!("updated {name} {} -> {}", previous.version, version.version);
    } else {
        println!("installed {name}@{}; run `ortzi {name}`", version.version);
    }
    if let Some(permissions) = &permissions {
        print_permissions(permissions);
    }

    Ok(())
}

fn print_permissions(permissions: &Permissions) {
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join(", ")
        }
    };
    println!("  runs: {}", list(&permissions.exec));
    println!("  env:  {}", list(&permissions.env));
    println!("  fs:   {}", list(&permissions.fs));
}

fn check_conflicts(installed: &Installed, name: &str, version: &Version) -> anyhow::Result<()> {
    let commands: Vec<&str> = std::iter::once(name)
        .chain(version.aliases.iter().map(String::as_str))
        .collect();
    for (other, package) in &installed.packages {
        if other == name {
            continue;
        }
        if package.binary == version.binary_name {
            bail!(
                "{name} installs binary {}, which {other} already provides",
                version.binary_name
            );
        }
        if let Some(clash) = commands
            .iter()
            .find(|c| **c == other || package.aliases.iter().any(|a| a == *c))
        {
            bail!("`ortzi {clash}` already runs {other}; uninstall it first");
        }
    }
    Ok(())
}

fn set_executable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn link_shim(store: &Store, name: &str, binary: &str) -> anyhow::Result<()> {
    let bin = store.bin_dir();
    std::fs::create_dir_all(&bin)?;
    let shim = bin.join(binary);
    let _ = std::fs::remove_file(&shim);
    std::os::unix::fs::symlink(
        Path::new("../packages")
            .join(name)
            .join("current")
            .join(binary),
        &shim,
    )?;
    Ok(())
}

pub async fn install(settings: &Settings, specs: &[String], force: bool) -> anyhow::Result<()> {
    let client = settings.reader().await?;
    let store = Store::open()?;
    let target = host_target()?;

    for spec in specs {
        let (name, version) = split_spec(spec);
        if name == SELF {
            self_update_with(&client, target, version, true).await?;
            continue;
        }
        let version = resolve(&client, name, version, target).await?;
        install_version(&client, &store, name, &version, target, force).await?;
    }

    Ok(())
}

pub fn uninstall(names: &[String]) -> anyhow::Result<()> {
    let store = Store::open()?;
    let mut installed = store.load()?;

    for name in names {
        let Some(package) = installed.packages.remove(name) else {
            bail!("{name} is not installed");
        };
        let _ = std::fs::remove_file(store.bin_dir().join(&package.binary));
        std::fs::remove_dir_all(store.package_dir(name))?;
        store.save(&installed)?;
        println!("uninstalled {name}");
    }

    Ok(())
}

pub async fn self_update(settings: &Settings, version: Option<&str>) -> anyhow::Result<()> {
    let client = settings.reader().await?;
    if !self_update_with(&client, host_target()?, version, version.is_some()).await? {
        println!("ortzi {} is up to date", env!("ORTZI_VERSION"));
    }
    Ok(())
}

async fn self_update_with(
    client: &Client,
    target: Target,
    version: Option<&str>,
    exact: bool,
) -> anyhow::Result<bool> {
    let release = resolve(client, SELF, version, target).await?;
    let running = env!("ORTZI_VERSION");
    let newer = match (
        semver::Version::parse(&release.version),
        semver::Version::parse(running),
    ) {
        (Ok(release), Ok(running)) => release > running,
        _ => release.version != running,
    };
    if release.version == running || (!newer && !exact) {
        return Ok(false);
    }

    let artifact = trust::verified_artifact(SELF, &release, target)?.artifact;
    if artifact.target != target {
        bail!("ortzi@{} has no {target} build", release.version);
    }

    let exe = std::env::current_exe()?.canonicalize()?;
    let staged = exe.with_file_name(format!(".ortzi-{}-{}", release.version, std::process::id()));
    eprintln!("downloading ortzi@{} ({target})", release.version);
    let sha = client
        .download(
            &format!(
                "/cli/v1/packages/{SELF}/versions/{}/artifacts/{target}",
                release.version
            ),
            &staged,
        )
        .await
        .with_context(|| format!("writing next to {}", exe.display()))?;
    if sha != artifact.sha256 {
        let _ = std::fs::remove_file(&staged);
        bail!("checksum mismatch for ortzi@{}", release.version);
    }
    set_executable(&staged)?;
    std::fs::rename(&staged, &exe).with_context(|| format!("replacing {}", exe.display()))?;

    println!("updated ortzi {running} -> {}", release.version);
    Ok(true)
}

pub async fn update(settings: &Settings, names: &[String]) -> anyhow::Result<()> {
    let store = Store::open()?;
    let installed = store.load()?;
    let client = settings.reader().await?;
    let target = host_target()?;
    let mut updated = 0;

    let everything = names.is_empty();
    if everything || names.iter().any(|n| n == SELF) {
        match self_update_with(&client, target, None, false).await {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(e) if everything => eprintln!("skipping ortzi self-update: {e:#}"),
            Err(e) => return Err(e),
        }
    }

    let names: Vec<String> = if everything {
        installed.packages.keys().cloned().collect()
    } else {
        names.iter().filter(|n| *n != SELF).cloned().collect()
    };

    let mut failed = Vec::new();
    for name in &names {
        let Some(current) = installed.packages.get(name) else {
            bail!("{name} is not installed");
        };
        match update_one(&client, &store, name, current, target).await {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(e) => {
                eprintln!("failed to update {name}: {e:#}");
                failed.push(name.as_str());
            }
        }
    }

    if !failed.is_empty() {
        bail!("failed to update {}", failed.join(", "));
    }
    if updated == 0 {
        println!("everything is up to date");
    }
    Ok(())
}

async fn update_one(
    client: &Client,
    store: &Store,
    name: &str,
    current: &InstalledPackage,
    target: Target,
) -> anyhow::Result<bool> {
    let latest = resolve(client, name, None, target).await?;
    let newer = match (
        semver::Version::parse(&latest.version),
        semver::Version::parse(&current.version),
    ) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => latest.version != current.version,
    };
    if newer {
        install_version(client, store, name, &latest, target, false).await?;
    }
    Ok(newer)
}

pub fn list() -> anyhow::Result<()> {
    let store = Store::open()?;
    let installed = store.load()?;
    if installed.packages.is_empty() {
        println!("nothing installed; try `ortzi search`");
        return Ok(());
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_BORDERS_ONLY)
        .set_header(["package", "version", "binary", "aliases"]);
    for (name, package) in &installed.packages {
        table.add_row([
            name.as_str(),
            &package.version,
            &package.binary,
            &package.aliases.join(", "),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn search(settings: &Settings, query: Option<&str>) -> anyhow::Result<()> {
    let client = settings.reader().await?;
    let path = match query {
        Some(q) => format!("/cli/v1/packages?q={}", urlencode(q)),
        None => "/cli/v1/packages".to_owned(),
    };
    let packages: Vec<Package> = client.get(&path).await?;
    let installed = Store::open()?.load()?;

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY).set_header([
        "package",
        "latest",
        "installed",
        "description",
    ]);
    for package in &packages {
        table.add_row([
            package.name.as_str(),
            package.latest.as_deref().unwrap_or("-"),
            if package.name == SELF {
                env!("ORTZI_VERSION")
            } else {
                installed
                    .packages
                    .get(&package.name)
                    .map_or("", |p| p.version.as_str())
            },
            &package.description,
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn info(settings: &Settings, name: &str) -> anyhow::Result<()> {
    let client = settings.reader().await?;
    let detail: PackageDetail = client.get(&format!("/cli/v1/packages/{name}")).await?;
    let package = &detail.package;

    println!("{}  {}", package.name, package.description);
    println!("binary   {}", package.binary_name);
    if !package.aliases.is_empty() {
        println!("aliases  {}", package.aliases.join(", "));
    }
    println!("latest   {}", package.latest.as_deref().unwrap_or("-"));
    if let Some(latest) = &package.latest
        && let Some(version) = client
            .find::<Version>(&format!("/cli/v1/packages/{name}/versions/{latest}"))
            .await?
    {
        match trust::verified_statement(name, &version) {
            Ok(statement) => {
                if let Some(permissions) = &statement.permissions {
                    println!("sandbox  wasm");
                    print_permissions(permissions);
                }
            }
            Err(e) => println!("signature {e:#}"),
        }
    }

    if !detail.versions.is_empty() {
        let mut table = Table::new();
        table
            .load_preset(UTF8_BORDERS_ONLY)
            .set_header(["version", "targets", "published", ""]);
        for version in &detail.versions {
            table.add_row([
                version.version.clone(),
                version
                    .targets
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                version
                    .published_at
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default(),
                if version.yanked {
                    "yanked".to_owned()
                } else {
                    String::new()
                },
            ]);
        }
        println!("{table}");
    }
    Ok(())
}

pub fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_split_on_at() {
        assert_eq!(split_spec("ff"), ("ff", None));
        assert_eq!(split_spec("ff@1.2.0"), ("ff", Some("1.2.0")));
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(urlencode("feature flags"), "feature%20flags");
        assert_eq!(urlencode("a&b"), "a%26b");
    }
}
