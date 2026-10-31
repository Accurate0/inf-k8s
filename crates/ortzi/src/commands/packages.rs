use std::path::Path;

use anyhow::{Context, bail};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use registry_api::{
    Package, PackageDetail, Permissions, Target, Version, validate_binary_name,
    validate_package_name, version_key,
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

pub async fn resolve(
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
        completions,
        env,
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
        .download_artifact(
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

    point_current(&package_dir, &version.version)?;

    let mut package = InstalledPackage {
        version: version.version.clone(),
        target,
        sha256: artifact.sha256.clone(),
        binary: version.binary_name.clone(),
        aliases: version.aliases.clone(),
        auth: version.auth.clone(),
        permissions: permissions.clone(),
        completions,
        env,
        pinned: false,
        previous: None,
    };
    let existing = installed.packages.get(name).cloned();
    if let Some(existing) = &existing {
        package = package.replacing(existing);
    }
    let kept = package.previous.as_ref().map(|p| p.version.clone());
    sync_shim(store, name, &package)?;
    installed.packages.insert(name.to_owned(), package);
    store.save(&installed)?;

    if let Some(existing) = existing {
        if existing.binary != version.binary_name {
            let _ = std::fs::remove_file(store.bin_dir().join(&existing.binary));
        }
        if let Some(older) = existing.previous
            && older.version != version.version
            && Some(&older.version) != kept.as_ref()
        {
            let _ = std::fs::remove_dir_all(package_dir.join(&older.version));
        }
        println!("updated {name} {} -> {}", existing.version, version.version);
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

fn point_current(package_dir: &Path, version: &str) -> anyhow::Result<()> {
    let staged = package_dir.join("current.tmp");
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(version, &staged)?;
    std::fs::rename(&staged, package_dir.join("current"))?;
    Ok(())
}

fn sync_shim(store: &Store, name: &str, package: &InstalledPackage) -> anyhow::Result<()> {
    if package.is_wasm() {
        let _ = std::fs::remove_file(store.bin_dir().join(&package.binary));
        return Ok(());
    }
    link_shim(store, name, &package.binary)
}

pub fn shim_target(name: &str, binary: &str) -> std::path::PathBuf {
    Path::new("../packages")
        .join(name)
        .join("current")
        .join(binary)
}

fn link_shim(store: &Store, name: &str, binary: &str) -> anyhow::Result<()> {
    let bin = store.bin_dir();
    std::fs::create_dir_all(&bin)?;
    let shim = bin.join(binary);
    let _ = std::fs::remove_file(&shim);
    std::os::unix::fs::symlink(shim_target(name, binary), &shim)?;
    Ok(())
}

pub async fn install(settings: &Settings, specs: &[String], force: bool) -> anyhow::Result<()> {
    let client = settings.reader().await?;
    let store = Store::open()?;
    let target = host_target()?;

    for spec in specs {
        let (name, version) = split_spec(spec);
        let installed = store.load()?;
        if name == SELF {
            if installed.self_pinned && version.is_none() && !force {
                bail!("ortzi is pinned; run `ortzi unpin ortzi` or name a version");
            }
            self_update_with(&client, target, version, true).await?;
            continue;
        }
        if let Some(current) = installed.packages.get(name)
            && current.pinned
            && version.is_none()
            && !force
        {
            bail!(
                "{name} is pinned at {}; run `ortzi unpin {name}` or name a version",
                current.version
            );
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

pub fn rollback(name: &str) -> anyhow::Result<()> {
    if name == SELF {
        return rollback_self();
    }
    let store = Store::open()?;
    let mut installed = store.load()?;
    let (name, current) = installed
        .resolve(name)
        .map(|(n, p)| (n.clone(), p.clone()))
        .with_context(|| format!("{name} is not installed"))?;
    let target = current
        .rolled_back()
        .with_context(|| format!("{name} has no previous version to roll back to"))?;

    let package_dir = store.package_dir(&name);
    let file = package_dir.join(&target.version).join(target.file_name());
    if !file.exists() {
        bail!(
            "{} is missing; run `ortzi install {name}@{}`",
            file.display(),
            target.version
        );
    }

    point_current(&package_dir, &target.version)?;
    sync_shim(&store, &name, &target)?;
    if current.binary != target.binary {
        let _ = std::fs::remove_file(store.bin_dir().join(&current.binary));
    }
    let hint = !target.pinned;
    let version = target.version.clone();
    installed.packages.insert(name.clone(), target);
    store.save(&installed)?;

    println!("rolled back {name} {} -> {version}", current.version);
    if hint {
        println!("`ortzi update` will move it forward again; run `ortzi pin {name}` to stop that");
    }
    Ok(())
}

fn backup_self(store: &Store, exe: &Path) -> anyhow::Result<()> {
    let backup = store.self_backup();
    let dir = backup.parent().context("backup path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let staged = backup.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::copy(exe, &staged)?;
    std::fs::rename(&staged, &backup)?;
    std::fs::write(backup.with_extension("version"), env!("ORTZI_VERSION"))?;
    Ok(())
}

fn rollback_self() -> anyhow::Result<()> {
    let store = Store::open()?;
    let backup = store.self_backup();
    if !backup.exists() {
        bail!("no previous ortzi to roll back to; one is kept from the next update");
    }
    let version = std::fs::read_to_string(backup.with_extension("version"))
        .unwrap_or_else(|_| "unknown".to_owned());

    let exe = std::env::current_exe()?.canonicalize()?;
    let staged = exe.with_file_name(format!(".ortzi-rollback-{}", std::process::id()));
    std::fs::copy(&backup, &staged)
        .with_context(|| format!("writing next to {}", exe.display()))?;
    set_executable(&staged)?;
    let held = store
        .tmp_dir()
        .join(format!("ortzi-{}", std::process::id()));
    std::fs::create_dir_all(store.tmp_dir())?;
    std::fs::copy(&exe, &held)?;
    std::fs::rename(&staged, &exe).with_context(|| format!("replacing {}", exe.display()))?;
    std::fs::rename(&held, &backup)?;
    std::fs::write(backup.with_extension("version"), env!("ORTZI_VERSION"))?;

    println!("rolled back ortzi {} -> {version}", env!("ORTZI_VERSION"));
    if !store.load()?.self_pinned {
        println!("`ortzi update` will move it forward again; run `ortzi pin ortzi` to stop that");
    }
    Ok(())
}

pub fn set_pinned(name: &str, pinned: bool) -> anyhow::Result<()> {
    let store = Store::open()?;
    let mut installed = store.load()?;
    let label = if pinned { "pinned" } else { "unpinned" };
    if name == SELF {
        installed.self_pinned = pinned;
        store.save(&installed)?;
        println!("{label} ortzi at {}", env!("ORTZI_VERSION"));
        return Ok(());
    }
    let (name, version) = {
        let resolved = installed
            .resolve(name)
            .map(|(n, _)| n.clone())
            .with_context(|| format!("{name} is not installed"))?;
        let package = installed
            .packages
            .get_mut(&resolved)
            .context("package vanished")?;
        package.pinned = pinned;
        (resolved, package.version.clone())
    };
    store.save(&installed)?;
    println!("{label} {name} at {version}");
    Ok(())
}

pub async fn self_update(settings: &Settings, version: Option<&str>) -> anyhow::Result<()> {
    if version.is_none() && Store::open()?.load()?.self_pinned {
        println!(
            "ortzi is pinned at {}; run `ortzi unpin ortzi`",
            env!("ORTZI_VERSION")
        );
        return Ok(());
    }
    let client = settings.reader().await?;
    if !self_update_with(&client, host_target()?, version, version.is_some()).await? {
        println!("ortzi {} is up to date", env!("ORTZI_VERSION"));
    }
    Ok(())
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(current),
    ) {
        (Ok(candidate), Ok(current)) => version_key(&candidate) > version_key(&current),
        _ => candidate != current,
    }
}

async fn self_update_with(
    client: &Client,
    target: Target,
    version: Option<&str>,
    exact: bool,
) -> anyhow::Result<bool> {
    let release = resolve(client, SELF, version, target).await?;
    let running = env!("ORTZI_VERSION");
    let newer = is_newer(&release.version, running);
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
        .download_artifact(
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
    backup_self(&Store::open()?, &exe)?;
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
    if (everything || names.iter().any(|n| n == SELF)) && installed.self_pinned {
        println!("ortzi is pinned at {}; skipping", env!("ORTZI_VERSION"));
    } else if everything || names.iter().any(|n| n == SELF) {
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
        if current.pinned {
            println!("{name} is pinned at {}; skipping", current.version);
            continue;
        }
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
    let newer = is_newer(&latest.version, &current.version);
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
    table.load_preset(UTF8_BORDERS_ONLY).set_header([
        "package", "version", "previous", "pinned", "binary", "aliases",
    ]);
    for (name, package) in &installed.packages {
        table.add_row([
            name.as_str(),
            &package.version,
            package
                .previous
                .as_ref()
                .map_or("-", |p| p.version.as_str()),
            if package.pinned { "yes" } else { "" },
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
    if package.archived {
        println!("archived; hidden from search and closed to new versions");
    }
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
mod revision_tests {
    use super::is_newer;

    #[test]
    fn revision_is_newer_than_its_release() {
        assert!(is_newer("2.7.2-2", "2.7.2"));
        assert!(is_newer("2.7.3", "2.7.2-2"));
        assert!(!is_newer("2.7.2", "2.7.2-2"));
        assert!(!is_newer("2.7.2-rc.1", "2.7.2"));
    }
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
