use std::path::Path;

use anyhow::bail;
use cynic::MutationBuilder;
use registry_api::{
    Artifact, BINARY_ARTIFACT, HarborAction, HarborToken, PACKAGES_PROJECT, Target,
    oci::{self, Manifest},
    package_repository,
    signing::{self, Envelope, Statement},
};

use crate::{
    client::{self, Client, human_size, sha256_file},
    graphql as gql,
    manifest::Loaded,
    remote,
    session::Settings,
};

struct Built {
    target: Target,
    path: std::path::PathBuf,
    sha: String,
}

async fn push_to_harbor(
    client: &Client,
    token: &HarborToken,
    name: &str,
    version: &str,
    binary: &str,
    built: &Built,
) -> anyhow::Result<Artifact> {
    let oci = remote::oci_client(token)?;
    let repository = package_repository(PACKAGES_PROJECT, name);

    let mut layer = oci
        .push_file(&repository, built.target.media_type(), &built.path)
        .await?;
    if layer.digest != oci::sha256_digest(&built.sha) {
        bail!("{} changed while it was being pushed", built.path.display());
    }
    layer
        .annotations
        .insert(oci::TITLE.to_owned(), built.target.file_name(binary));
    oci.push_bytes(&repository, oci::EMPTY, oci::EMPTY_JSON)
        .await?;

    let manifest = serde_json::to_vec(&Manifest::artifact(BINARY_ARTIFACT, vec![layer]))?;
    let digest = oci::digest(&manifest);
    oci.put_manifest(&repository, &digest, oci::MANIFEST, &manifest)
        .await?;

    let registered = client
        .graph()
        .run(gql::RegisterArtifact::build(
            gql::RegisterArtifactVariables {
                package: name.to_owned(),
                version: version.to_owned(),
                target: client::target_in(built.target),
                manifest: digest,
            },
        ))
        .await?
        .registry_register_artifact;
    Ok(client::artifact(registered))
}

fn envelope_in(envelope: Envelope) -> gql::EnvelopeInput {
    gql::EnvelopeInput {
        payload: envelope.payload,
        signature: envelope.signature,
        key_id: envelope.key_id,
    }
}

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
        "publishing {name}@{version} through {} (auth: {via})",
        settings.graph
    );

    if let Some(existing) = client.version(&name, &version).await?
        && existing.status == "published"
    {
        if options.fail_if_exists {
            bail!("{name}@{version} is already published; bump the version");
        }
        println!("{name}@{version} is already published; nothing to do");
        return Ok(());
    }

    for (dependency, req) in &loaded.manifest.dependencies {
        let range = registry_api::requirement(req).map_err(anyhow::Error::msg)?;
        let published = client.package(dependency).await?.is_some_and(|package| {
            package
                .versions
                .iter()
                .any(|v| v.yanked.is_none() && registry_api::satisfies(&range, &v.version))
        });
        if !published {
            bail!("{name} depends on {dependency} {req}, which has no matching published version");
        }
    }

    let mut built = Vec::new();
    for target in &targets {
        let path = loaded.build(*target, &version).await?;
        let sha = sha256_file(&path).await?;
        built.push(Built {
            target: *target,
            path,
            sha,
        });
    }

    let draft = gql::CreateVersionInput {
        package: name.clone(),
        version: version.clone(),
        targets: declared.iter().copied().map(client::target_in).collect(),
        description: Some(loaded.manifest.package.description.clone()),
        binary_name: Some(loaded.binary().to_owned()),
        aliases: Some(loaded.manifest.package.aliases.clone()),
        auth: loaded.manifest.auth.as_ref().map(|a| {
            let auth = a.resolve();
            gql::PackageAuthInput {
                issuer: auth.issuer,
                client_id: auth.client_id,
                scopes: auth.scopes,
                token_env: auth.token_env,
                lazy: auth.lazy,
            }
        }),
    };
    let created = client
        .graph()
        .run(gql::CreateVersion::build(gql::CreateVersionVariables {
            input: draft,
        }))
        .await;
    let created = match created {
        Ok(created) => created.registry_create_version,
        Err(e) if client::not_found(&e) => {
            bail!(
                "package {name} does not exist in the registry; an admin needs to run\n  \
                 ortzi registry package create {name} --forgejo-repo <owner/repo>"
            );
        }
        Err(e) => return Err(e),
    };
    eprintln!("{name}@{} is a {}", created.version, created.status);

    let token = remote::exchange(
        &client,
        vec![remote::access(
            PACKAGES_PROJECT,
            &[HarborAction::Push],
            Some(&name),
        )],
    )
    .await?;
    let result = finish(&client, &token, &loaded, &options, &name, &version, &built).await;
    remote::revoke(&client, &token).await;
    result
}

async fn finish(
    client: &Client,
    token: &HarborToken,
    loaded: &Loaded,
    options: &Options<'_>,
    name: &str,
    version: &str,
    built: &[Built],
) -> anyhow::Result<()> {
    let (name, version) = (name.to_owned(), version.to_owned());
    for item in built {
        let artifact =
            push_to_harbor(client, token, &name, &version, loaded.binary(), item).await?;
        eprintln!(
            "uploaded {} ({}, harbor)",
            item.target,
            human_size(artifact.size)
        );
    }

    if options.no_finalize {
        println!(
            "uploaded {name}@{version}; finalise with `ortzi publish` once every target is uploaded"
        );
        return Ok(());
    }

    let draft = client
        .version(&name, &version)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the draft {name}@{version} disappeared"))?;
    let artifacts: Vec<Artifact> = draft.artifacts.into_iter().map(client::artifact).collect();
    for item in built {
        if !artifacts
            .iter()
            .any(|a| a.target == item.target && a.sha256 == item.sha)
        {
            bail!(
                "the registry's {} artifact doesn't match what was just uploaded",
                item.target
            );
        }
    }

    let key = signing::generate();
    let certificate = client
        .graph()
        .run(gql::IssueSigningCertificate::build(
            gql::CertificateVariables {
                package: name.clone(),
                public_key: signing::encode_key(key.verifying_key()),
            },
        ))
        .await?
        .registry_issue_signing_certificate;
    let statement = signing::sign(
        &key,
        &Statement {
            package: name.clone(),
            version: version.clone(),
            artifacts,
            permissions: loaded.manifest.permissions.clone(),
            completions: loaded.manifest.package.completions,
            env: loaded.manifest.package.env.clone(),
            dependencies: loaded.manifest.dependencies.clone(),
        },
    );

    let published = client
        .graph()
        .run(gql::PublishVersion::build(gql::PublishVariables {
            package: name.clone(),
            version: version.clone(),
            statement: envelope_in(statement),
            certificate: gql::EnvelopeInput {
                payload: certificate.payload,
                signature: certificate.signature,
                key_id: certificate.key_id,
            },
        }))
        .await?
        .registry_publish_version;
    println!(
        "published {name}@{}; install with `ortzi install {name}`",
        published.version
    );
    Ok(())
}
