use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use registry_api::validate_package_name;

use crate::manifest::{self, Build, Kind, Manifest, PackageSection};

const WORKFLOW_TEMPLATE: &str = include_str!("../../templates/forgejo-workflow.yaml");

pub struct Options<'a> {
    pub path: &'a Path,
    pub name: Option<&'a str>,
    pub force: bool,
    pub no_workflow: bool,
}

pub fn init(options: Options<'_>) -> anyhow::Result<()> {
    let dir = options.path.canonicalize()?;
    let manifest_path = dir.join(manifest::FILE_NAME);
    if manifest_path.exists() && !options.force {
        bail!(
            "{} already exists (use --force to overwrite)",
            manifest_path.display()
        );
    }

    let cargo_toml = dir.join("Cargo.toml");
    let (detected_name, binary, version) = if cargo_toml.exists() {
        detect_cargo(&cargo_toml)?
    } else {
        (None, None, None)
    };

    let name = options
        .name
        .map(str::to_owned)
        .or(detected_name)
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .context("could not work out a package name; pass --name")?;
    validate_package_name(&name).map_err(|e| anyhow::anyhow!("{e}; pass --name"))?;

    let kind = if cargo_toml.exists() {
        Kind::Cargo
    } else {
        Kind::Prebuilt
    };
    let mut build = Build {
        kind,
        ..Build::default()
    };
    if kind == Kind::Prebuilt {
        for target in &build.targets {
            build
                .prebuilt
                .insert(*target, format!("dist/{name}-${{version}}-{target}"));
        }
    }

    let manifest = Manifest {
        package: PackageSection {
            binary: binary.filter(|b| b != &name),
            name: name.clone(),
            description: String::new(),
            aliases: Vec::new(),
            version: Some(version.unwrap_or_else(|| "0.1.0".to_owned())),
        },
        auth: None,
        build,
    };
    std::fs::write(&manifest_path, serde_yaml::to_string(&manifest)?)?;
    println!("wrote {}", manifest_path.display());

    let repo = git_repo(&dir);
    if !options.no_workflow {
        match &repo {
            Some((root, _)) => {
                let workflow = write_workflow(root, &dir, &name, options.force)?;
                println!("wrote {}", workflow.display());
            }
            None => eprintln!("not in a git repository; skipping the Forgejo workflow"),
        }
    }

    let repository = repo
        .and_then(|(_, repository)| repository)
        .unwrap_or_else(|| "<owner/repo>".to_owned());
    println!();
    println!("next: ask a registry admin to create the package and trust this repo:");
    println!(
        "  ortzi registry package create {name} --forgejo-repo {repository} --ref refs/heads/main --workflow publish-cli-{name}.yaml"
    );
    println!("then push to main, or publish by hand with `ortzi publish`");
    Ok(())
}

fn detect_cargo(
    cargo_toml: &Path,
) -> anyhow::Result<(Option<String>, Option<String>, Option<String>)> {
    let metadata = manifest::cargo_metadata(cargo_toml)?;
    let canonical = cargo_toml.canonicalize()?;
    let Some(package) = metadata
        .packages
        .into_iter()
        .find(|p| p.manifest_path.canonicalize().ok().as_deref() == Some(canonical.as_path()))
    else {
        return Ok((None, None, None));
    };

    let bins: Vec<String> = package
        .targets
        .iter()
        .filter(|t| t.kind.iter().any(|k| k == "bin"))
        .map(|t| t.name.clone())
        .collect();
    let binary = match bins.as_slice() {
        [] => bail!("{} has no binary target; add a [[bin]] first", package.name),
        [only] => only.clone(),
        many => bins
            .iter()
            .find(|b| **b == package.name)
            .cloned()
            .with_context(|| {
                format!("several binaries ({}); set package.binary", many.join(", "))
            })?,
    };

    let name = validate_package_name(&package.name)
        .is_ok()
        .then(|| package.name.clone());
    Ok((name, Some(binary), Some(package.version)))
}

fn git_repo(dir: &Path) -> Option<(PathBuf, Option<String>)> {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let root = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?);
    let repository =
        git(&["remote", "get-url", "origin"]).and_then(|url| repository_from_remote(&url));
    Some((root, repository))
}

fn repository_from_remote(url: &str) -> Option<String> {
    let path = url
        .split_once("://")
        .map(|(_, rest)| rest.split_once('/').map_or("", |(_, path)| path))
        .or_else(|| url.split_once(':').map(|(_, path)| path))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = path.rsplitn(3, '/');
    let repo = parts.next()?;
    let owner = parts.next()?;
    (!owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}"))
}

fn write_workflow(root: &Path, dir: &Path, name: &str, force: bool) -> anyhow::Result<PathBuf> {
    let relative = dir.strip_prefix(root).unwrap_or(Path::new(""));
    let relative = relative.to_string_lossy();
    let (paths, publish_dir) = if relative.is_empty() {
        ("\"**\"".to_owned(), ".".to_owned())
    } else {
        (format!("{relative}/**"), relative.into_owned())
    };

    let file = format!("publish-cli-{name}.yaml");
    let path = root.join(".forgejo/workflows").join(&file);
    if path.exists() && !force {
        bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }

    let contents = WORKFLOW_TEMPLATE
        .replace("{name}", name)
        .replace("{paths}", &paths)
        .replace("{workflow}", &file)
        .replace("{dir}", &publish_dir);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, contents)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        for url in [
            "ssh://git@git.anurag.sh/anurag/k8s.git",
            "https://git.anurag.sh/anurag/k8s",
            "git@git.anurag.sh:anurag/k8s.git",
            "https://git.anurag.sh/anurag/k8s.git/",
        ] {
            assert_eq!(
                repository_from_remote(url).as_deref(),
                Some("anurag/k8s"),
                "{url}"
            );
        }
        assert_eq!(repository_from_remote("nonsense"), None);
    }

    #[test]
    fn workflow_template_fills_in() {
        let root = std::env::temp_dir().join(format!("ortzi-init-{}", std::process::id()));
        let dir = root.join("tools/foo");
        std::fs::create_dir_all(&dir).unwrap();

        let path = write_workflow(&root, &dir, "foo", false).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        assert!(contents.contains("name: Publish foo"));
        assert!(contents.contains("- tools/foo/**"));
        assert!(contents.contains(".forgejo/workflows/publish-cli-foo.yaml"));
        assert!(contents.contains("run: ortzi publish tools/foo"));
        assert!(contents.contains("enable-openid-connect: true"));
    }
}
