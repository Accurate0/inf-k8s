use std::path::Path;

use anyhow::bail;
use registry_api::{
    Artifact, CreateVersion, ECOSYSTEM_CLI, Target, Version,
    signing::{self, CertificateRequest, Envelope, PublishRequest, Statement},
};
use reqwest::{Method, StatusCode};

use crate::{
    client::{api_error, human_size, sha256_file},
    manifest::Loaded,
    session::Settings,
};

pub struct Options<'a> {
    pub path: &'a Path,
    pub targets: &'a [Target],
    pub dry_run: bool,
    pub no_finalize: bool,
    pub fail_if_exists: bool,
}

pub async fn publish(settings: &Settings, options: Options<'_>) -> anyhow::Result<()> {
    let loaded = Loaded::load(options.path)?;
    let name = loaded.name().to_owned();
    let version = loaded.version()?;
    let declared = &loaded.manifest.build.targets;

    let targets: Vec<Target> = if options.targets.is_empty() {
        declared.clone()
    } else {
        for target in options.targets {
            if !declared.contains(target) {
                bail!("{target} is not in build.targets");
            }
        }
        options.targets.to_vec()
    };

    if options.dry_run {
        for target in &targets {
            let artifact = loaded.build(*target, &version).await?;
            println!(
                "{target}: {} ({})",
                artifact.display(),
                sha256_file(&artifact).await?
            );
        }
        println!("dry run: {name}@{version} not published");
        return Ok(());
    }

    let (client, via) = settings.publisher().await?;
    eprintln!(
        "publishing {name}@{version} to {} (auth: {via})",
        settings.registry
    );

    let base = format!("/cli/v1/packages/{name}/versions");
    if let Some(existing) = client.find::<Version>(&format!("{base}/{version}")).await?
        && existing.status == "published"
    {
        if options.fail_if_exists {
            bail!("{name}@{version} is already published; bump the version");
        }
        println!("{name}@{version} is already published; nothing to do");
        return Ok(());
    }

    let mut built = Vec::new();
    for target in &targets {
        let path = loaded.build(*target, &version).await?;
        let sha = sha256_file(&path).await?;
        built.push((*target, path, sha));
    }

    let draft = CreateVersion {
        version: version.clone(),
        targets: declared.clone(),
        description: Some(loaded.manifest.package.description.clone()),
        binary_name: Some(loaded.binary().to_owned()),
        aliases: Some(loaded.manifest.package.aliases.clone()),
        auth: loaded.manifest.auth.as_ref().map(|a| a.resolve()),
    };
    if let Err(e) = client.send::<_, Version>(Method::POST, &base, &draft).await {
        if api_error(&e).is_some_and(|e| e.status == StatusCode::NOT_FOUND) {
            bail!(
                "package {name} does not exist in the registry; an admin needs to run\n  \
                 ortzi registry package create {name} --forgejo-repo <owner/repo>"
            );
        }
        return Err(e);
    }

    for (target, path, sha) in &built {
        let artifact: Artifact = client
            .upload(&format!("{base}/{version}/artifacts/{target}"), path, sha)
            .await?;
        eprintln!("uploaded {target} ({})", human_size(artifact.size));
    }

    if options.no_finalize {
        println!(
            "uploaded {name}@{version}; finalise with `ortzi publish` once every target is uploaded"
        );
        return Ok(());
    }

    let draft: Version = client.get(&format!("{base}/{version}")).await?;
    for (target, _, sha) in &built {
        if !draft
            .artifacts
            .iter()
            .any(|a| a.target == *target && &a.sha256 == sha)
        {
            bail!("the registry's {target} artifact doesn't match what was just uploaded");
        }
    }

    let key = signing::generate();
    let certificate: Envelope = client
        .send(
            Method::POST,
            "/api/v1/signing/certificate",
            &CertificateRequest {
                ecosystem: ECOSYSTEM_CLI.to_owned(),
                package: name.clone(),
                public_key: signing::encode_key(key.verifying_key()),
            },
        )
        .await?;
    let statement = signing::sign(
        &key,
        &Statement {
            package: name.clone(),
            version: version.clone(),
            artifacts: draft.artifacts,
            permissions: loaded.manifest.permissions.clone(),
            completions: loaded.manifest.package.completions,
            env: loaded.manifest.package.env.clone(),
        },
    );

    let published: Version = client
        .send(
            Method::POST,
            &format!("{base}/{version}/publish"),
            &PublishRequest {
                statement,
                certificate,
            },
        )
        .await?;
    println!(
        "published {name}@{}; install with `ortzi install {name}`",
        published.version
    );
    Ok(())
}
