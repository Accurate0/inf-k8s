use std::{collections::BTreeMap, path::Path, time::Duration};

use anyhow::{Context, bail};
use registry_api::{
    Artifact, CATALOG_MEDIA_TYPE, CATALOG_PROJECT, CATALOG_REPOSITORY, Catalog, CatalogRef,
    HarborAccess, HarborAction, HarborToken, HarborTokenRequest, MAX_CATALOG_BYTES,
    PACKAGES_PROJECT, Package, PackageDetail, SIGNATURES_ARTIFACT, SIGNATURES_MEDIA_TYPE, Target,
    Version, VersionSummary,
    oci::{self, Credentials, Manifest},
    signing::{self, Envelope, Signatures},
    version_key,
};
use reqwest::{Method, StatusCode};

use crate::{
    client::{self, Client},
    trust,
};

const MAX_SIGNATURES_BYTES: u64 = 1024 * 1024;

pub struct Remote {
    api: Client,
    oci: Option<Oci>,
    token: Option<HarborToken>,
}

struct Oci {
    client: oci::Client,
    catalog: Catalog,
}

pub fn access(project: &str, actions: &[HarborAction], package: Option<&str>) -> HarborAccess {
    HarborAccess {
        project: project.to_owned(),
        actions: actions.to_vec(),
        package: package.map(str::to_owned),
    }
}

pub async fn exchange(
    api: &Client,
    access: Vec<HarborAccess>,
) -> anyhow::Result<Option<HarborToken>> {
    match api
        .send::<_, HarborToken>(
            Method::POST,
            "/api/v1/harbor/token",
            &HarborTokenRequest { access },
        )
        .await
    {
        Ok(token) => Ok(Some(token)),
        Err(e)
            if client::api_error(&e)
                .is_some_and(|e| e.status == StatusCode::NOT_FOUND || e.code == "harbor") =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

pub async fn revoke(api: &Client, token: &HarborToken) {
    let path = format!(
        "/api/v1/harbor/token/{}",
        crate::commands::packages::urlencode(&token.username)
    );
    if let Err(e) = api.delete(&path).await {
        eprintln!(
            "warning: could not revoke harbor robot {}: {e:#}",
            token.username
        );
    }
}

pub fn oci_client(token: &HarborToken) -> anyhow::Result<oci::Client> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("ortzi/", env!("ORTZI_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(120))
        .build()?;
    Ok(oci::Client::new(
        http,
        &format!("https://{}", token.registry),
        Some(Credentials {
            username: token.username.clone(),
            password: token.password.clone(),
        }),
    )?)
}

async fn load_catalog(client: &oci::Client, current: &CatalogRef) -> anyhow::Result<Catalog> {
    let digest = current
        .digest
        .as_deref()
        .context("the registry has no catalog yet")?;
    let fetched = client
        .manifest(CATALOG_REPOSITORY, digest)
        .await?
        .with_context(|| format!("catalog {digest} is missing from harbor"))?;
    let manifest: Manifest =
        serde_json::from_slice(&fetched.body).context("unreadable catalog manifest")?;
    let [layer] = manifest.layers.as_slice() else {
        bail!("the catalog manifest should have exactly one layer");
    };
    if layer.media_type != CATALOG_MEDIA_TYPE {
        bail!("unexpected catalog layer type {}", layer.media_type);
    }

    let body = client
        .blob_bytes(CATALOG_REPOSITORY, &layer.digest, MAX_CATALOG_BYTES)
        .await?;
    let envelope: Envelope = serde_json::from_slice(&body).context("unreadable catalog")?;
    let catalog = signing::verify_catalog(&trust::roots()?, &envelope)
        .map_err(|e| anyhow::anyhow!("the registry catalog failed signature verification: {e}"))?;

    if catalog.seq != current.seq {
        bail!(
            "catalog {digest} is seq {}, but the registry says {}",
            catalog.seq,
            current.seq
        );
    }
    Ok(catalog)
}

impl Remote {
    pub async fn open(api: Client) -> anyhow::Result<Self> {
        let legacy = |api| {
            Ok(Self {
                api,
                oci: None,
                token: None,
            })
        };

        let current = match api.find::<CatalogRef>("/cli/v1/catalog").await? {
            Some(current) if current.digest.is_some() => current,
            _ => return legacy(api),
        };
        let Some(token) = exchange(
            &api,
            vec![
                access(CATALOG_PROJECT, &[HarborAction::Pull], None),
                access(PACKAGES_PROJECT, &[HarborAction::Pull], None),
            ],
        )
        .await?
        else {
            return legacy(api);
        };

        let client = oci_client(&token)?;
        let catalog = match load_catalog(&client, &current).await {
            Ok(catalog) => catalog,
            Err(e) => {
                revoke(&api, &token).await;
                return Err(e);
            }
        };
        Ok(Self {
            api,
            oci: Some(Oci { client, catalog }),
            token: Some(token),
        })
    }

    pub async fn close(self) {
        if let Some(token) = &self.token {
            revoke(&self.api, token).await;
        }
    }

    pub async fn resolve(
        &self,
        name: &str,
        version: Option<&str>,
        target: Target,
    ) -> anyhow::Result<Version> {
        let Some(oci) = &self.oci else {
            return crate::commands::packages::resolve(&self.api, name, version, target).await;
        };

        let found = match version {
            Some(version) => oci.catalog.version(name, version),
            None => oci.catalog.latest(name, Some(target), false),
        };
        let (version, entry) = found.with_context(|| match version {
            Some(version) => format!("{name}@{version} not found"),
            None => format!("no published {name} for {target}; see `ortzi search`"),
        })?;

        let signatures = oci
            .signatures(&oci.catalog.repository(name), &entry.signatures)
            .await
            .with_context(|| format!("fetching signatures for {name}@{version}"))?;
        Ok(entry.to_version(name, version, signatures))
    }

    pub async fn latest_versions(
        &self,
        names: &[&str],
        target: Target,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        let Some(oci) = &self.oci else {
            return crate::commands::packages::latest_versions(&self.api, names, target).await;
        };
        Ok(names
            .iter()
            .filter_map(|name| {
                let (version, _) = oci.catalog.latest(name, Some(target), false)?;
                Some(((*name).to_owned(), version.to_owned()))
            })
            .collect())
    }

    pub async fn download(
        &self,
        name: &str,
        version: &Version,
        artifact: &Artifact,
        dest: &Path,
    ) -> anyhow::Result<String> {
        let Some(oci) = &self.oci else {
            return self
                .api
                .download_artifact(
                    &format!(
                        "/cli/v1/packages/{name}/versions/{}/artifacts/{}",
                        version.version, artifact.target
                    ),
                    dest,
                )
                .await;
        };

        oci.client
            .blob_to_file(
                &oci.catalog.repository(name),
                &oci::sha256_digest(&artifact.sha256),
                dest,
            )
            .await?;
        Ok(artifact.sha256.clone())
    }

    pub async fn search(&self, query: Option<&str>) -> anyhow::Result<Vec<Package>> {
        let Some(oci) = &self.oci else {
            let path = match query {
                Some(q) => format!(
                    "/cli/v1/packages?q={}",
                    crate::commands::packages::urlencode(q)
                ),
                None => "/cli/v1/packages".to_owned(),
            };
            return self.api.get(&path).await;
        };

        let query = query.map(str::to_lowercase);
        Ok(oci
            .catalog
            .packages
            .iter()
            .filter(|(_, p)| !p.archived)
            .filter(|(name, p)| {
                query
                    .as_deref()
                    .is_none_or(|q| name.contains(q) || p.description.to_lowercase().contains(q))
            })
            .map(|(name, _)| oci.package(name))
            .collect())
    }

    pub async fn detail(&self, name: &str) -> anyhow::Result<PackageDetail> {
        let Some(oci) = &self.oci else {
            return self.api.get(&format!("/cli/v1/packages/{name}")).await;
        };

        let package = oci
            .catalog
            .packages
            .get(name)
            .with_context(|| format!("package {name} not found"))?;
        let mut versions: Vec<VersionSummary> = package
            .versions
            .iter()
            .map(|(version, v)| VersionSummary {
                version: version.clone(),
                targets: v.targets.clone(),
                published_at: Some(v.published_at),
                yanked: v.yanked.is_some(),
            })
            .collect();
        versions.sort_by_key(|v| {
            std::cmp::Reverse(
                semver::Version::parse(&v.version)
                    .ok()
                    .map(|s| version_key(&s)),
            )
        });
        Ok(PackageDetail {
            package: oci.package(name),
            versions,
        })
    }
}

impl Oci {
    fn package(&self, name: &str) -> Package {
        let package = &self.catalog.packages[name];
        Package {
            name: name.to_owned(),
            description: package.description.clone(),
            binary_name: package.binary_name.clone(),
            aliases: package.aliases.clone(),
            latest: self
                .catalog
                .latest(name, None, false)
                .map(|(v, _)| v.to_owned()),
            created_at: self.catalog.generated_at,
            archived: package.archived,
        }
    }

    async fn signatures(&self, repository: &str, digest: &str) -> anyhow::Result<Signatures> {
        let fetched = self
            .client
            .manifest(repository, digest)
            .await?
            .with_context(|| format!("{repository}@{digest} is missing"))?;
        let manifest: Manifest = serde_json::from_slice(&fetched.body)?;
        if manifest.artifact_type.as_deref() != Some(SIGNATURES_ARTIFACT) {
            bail!("{repository}@{digest} is not a signatures artifact");
        }
        let [layer] = manifest.layers.as_slice() else {
            bail!("{repository}@{digest} should have exactly one layer");
        };
        if layer.media_type != SIGNATURES_MEDIA_TYPE {
            bail!("unexpected signatures layer type {}", layer.media_type);
        }
        let body = self
            .client
            .blob_bytes(repository, &layer.digest, MAX_SIGNATURES_BYTES)
            .await?;
        Ok(serde_json::from_slice(&body)?)
    }
}
