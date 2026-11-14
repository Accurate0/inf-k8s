use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use flate2::{Compression, GzBuilder};
use registry_api::{
    HarborAction,
    oci::{self, Manifest},
};

use super::harbor;
use crate::{client::Client, remote};

const ARTIFACT_TYPE: &str = "application/vnd.ortzi.manifests.v1";
const LAYER: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
const REVISION: &str = "org.opencontainers.image.revision";

struct Target<'a> {
    host: &'a str,
    project: &'a str,
    repository: &'a str,
}

fn parse_reference(reference: &str) -> anyhow::Result<Target<'_>> {
    let Some(rest) = reference.strip_prefix("oci://") else {
        bail!("{reference} is not oci://<host>/<project>/<name>");
    };
    let Some((host, repository)) = rest.split_once('/') else {
        bail!("{reference} has no repository");
    };
    let Some((project, name)) = repository.split_once('/') else {
        bail!("{reference} is not oci://<host>/<project>/<name>");
    };

    if host.is_empty() || project.is_empty() || name.is_empty() {
        bail!("{reference} is not oci://<host>/<project>/<name>");
    }

    Ok(Target {
        host,
        project,
        repository,
    })
}

fn files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];

    while let Some(dir) = pending.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();

            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                found.push(path.strip_prefix(root)?.to_path_buf());
            } else {
                bail!("{} is not a regular file", path.display());
            }
        }
    }

    found.sort();
    Ok(found)
}

fn archive(root: &Path) -> anyhow::Result<Vec<u8>> {
    let gzip = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    let mut tar = tar::Builder::new(gzip);
    tar.mode(tar::HeaderMode::Deterministic);

    for path in files(root)? {
        let bytes = std::fs::read(root.join(&path))
            .with_context(|| format!("reading {}", path.display()))?;

        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        tar.append_data(&mut header, &path, bytes.as_slice())?;
    }

    Ok(tar.into_inner()?.finish()?)
}

pub async fn push(
    api: &Client,
    dir: &Path,
    reference: &str,
    tags: &[String],
    revision: Option<&str>,
    sign: bool,
) -> anyhow::Result<()> {
    let target = parse_reference(reference)?;
    if tags.is_empty() {
        bail!("pass at least one --tag");
    }

    let layer = archive(dir)?;

    let access = remote::access(
        target.project,
        &[HarborAction::Pull, HarborAction::Push],
        None,
    );
    let Some(token) = remote::exchange(api, vec![access]).await? else {
        bail!("the registry has no harbor token exchange");
    };

    let pushed = upload(&token, &target, &layer, tags, revision).await;
    remote::revoke(api, &token).await;
    let digest = pushed?;

    println!("pushed {reference}@{digest} ({})", tags.join(", "));

    if sign {
        harbor::sign(
            api,
            &format!("{}/{}@{digest}", target.host, target.repository),
        )
        .await?;
    }

    Ok(())
}

async fn upload(
    token: &registry_api::HarborToken,
    target: &Target<'_>,
    layer: &[u8],
    tags: &[String],
    revision: Option<&str>,
) -> anyhow::Result<String> {
    if token.registry != target.host {
        bail!(
            "{} is not the ortzi registry ({})",
            target.host,
            token.registry
        );
    }

    let oci = remote::oci_client(token)?;
    let descriptor = oci.push_bytes(target.repository, LAYER, layer).await?;
    oci.push_bytes(target.repository, oci::EMPTY, oci::EMPTY_JSON)
        .await?;

    let mut manifest = Manifest::artifact(ARTIFACT_TYPE, vec![descriptor]);
    if let Some(revision) = revision {
        manifest
            .annotations
            .insert(REVISION.to_owned(), revision.to_owned());
    }

    let body = serde_json::to_vec(&manifest)?;
    let digest = oci::digest(&body);
    for tag in tags {
        oci.put_manifest(target.repository, tag, oci::MANIFEST, &body)
            .await?;
    }

    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_parse() {
        let target = parse_reference("oci://registry.anurag.sh/manifests/k8s-schemas").unwrap();
        assert_eq!(target.host, "registry.anurag.sh");
        assert_eq!(target.project, "manifests");
        assert_eq!(target.repository, "manifests/k8s-schemas");
        assert!(parse_reference("registry.anurag.sh/manifests/x").is_err());
        assert!(parse_reference("oci://registry.anurag.sh/manifests").is_err());
        assert!(parse_reference("oci://registry.anurag.sh//x").is_err());
    }

    #[test]
    fn archives_are_reproducible() {
        let dir = std::env::temp_dir().join(format!("ortzi-artifact-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        std::fs::write(dir.join("kustomization.yaml"), "resources: []\n").unwrap();
        std::fs::write(dir.join("manifests/a.yaml"), "kind: ConfigMap\n").unwrap();

        let first = archive(&dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(dir.join("manifests/a.yaml"), "kind: ConfigMap\n").unwrap();
        let second = archive(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(oci::digest(&first), oci::digest(&second));
    }
}
