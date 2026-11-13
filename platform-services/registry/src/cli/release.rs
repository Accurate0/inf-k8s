use std::collections::BTreeMap;

use registry_api::{
    BINARY_ARTIFACT, PACKAGE_ARTIFACT, SIGNATURES_ARTIFACT, SIGNATURES_MEDIA_TYPE, Target,
    oci::{self, Descriptor, Index, Manifest, Platform},
    package_repository,
    signing::Signatures,
};

use super::{
    cosign,
    db::{ArtifactRow, VersionRow},
};
use crate::{
    error::{ApiError, Result},
    harbor::Harbor,
    state::AppState,
};

pub const OCI_KEY_PREFIX: &str = "oci:";

pub struct Released {
    pub index: String,
    pub signatures: String,
}

pub fn cosign_registry(state: &AppState) -> &str {
    state
        .config
        .harbor
        .as_ref()
        .map_or("", |h| h.registry.as_str())
}

pub fn oci_manifest(object_key: &str) -> Option<&str> {
    object_key.strip_prefix(OCI_KEY_PREFIX)
}

pub fn validate_binary_manifest(
    manifest: &Manifest,
    target: Target,
    max_bytes: u64,
) -> Result<Descriptor> {
    let invalid = |reason: &str| ApiError::Unprocessable(format!("{target} manifest {reason}"));

    if manifest.media_type != oci::MANIFEST {
        return Err(invalid("is not an OCI image manifest"));
    }
    if manifest.artifact_type.as_deref() != Some(BINARY_ARTIFACT) {
        return Err(invalid(&format!(
            "must have artifactType {BINARY_ARTIFACT}"
        )));
    }
    let [layer] = manifest.layers.as_slice() else {
        return Err(invalid("must have exactly one layer"));
    };
    if layer.media_type != target.media_type() {
        return Err(invalid(&format!(
            "layer must be {}, not {}",
            target.media_type(),
            layer.media_type
        )));
    }
    let hex = layer
        .digest
        .strip_prefix("sha256:")
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| invalid("layer digest must be sha256"))?;
    if hex != hex.to_ascii_lowercase() {
        return Err(invalid("layer digest must be lowercase"));
    }
    if layer.size > max_bytes {
        return Err(ApiError::TooLarge(max_bytes));
    }
    Ok(layer.clone())
}

async fn put_by_digest<T: serde::Serialize>(
    harbor: &Harbor,
    repository: &str,
    media_type: &str,
    value: &T,
) -> Result<Descriptor> {
    let body = serde_json::to_vec(value).expect("manifests always serialise");
    let digest = oci::digest(&body);
    harbor
        .oci()
        .put_manifest(repository, &digest, media_type, &body)
        .await?;
    Ok(Descriptor::new(media_type, digest, body.len() as u64))
}

async fn binary_manifest(
    state: &AppState,
    harbor: &Harbor,
    repository: &str,
    binary_name: &str,
    artifact: &ArtifactRow,
) -> Result<Descriptor> {
    let target: Target = artifact.target.parse().map_err(ApiError::BadRequest)?;

    if let Some(digest) = oci_manifest(&artifact.object_key) {
        let fetched = harbor
            .oci()
            .manifest(repository, digest)
            .await?
            .ok_or_else(|| {
                ApiError::Harbor(format!("{repository}@{digest} is missing from harbor"))
            })?;
        return Ok(Descriptor::new(
            oci::MANIFEST,
            fetched.digest,
            fetched.body.len() as u64,
        ));
    }

    let stream = state.storage.get_stream(&artifact.object_key).await?;
    let mut layer = harbor
        .oci()
        .push_stream(
            repository,
            target.media_type(),
            &oci::sha256_digest(&artifact.sha256),
            artifact.size as u64,
            stream,
        )
        .await?;
    layer
        .annotations
        .insert(oci::TITLE.to_owned(), target.file_name(binary_name));

    harbor
        .oci()
        .push_bytes(repository, oci::EMPTY, oci::EMPTY_JSON)
        .await?;
    put_by_digest(
        harbor,
        repository,
        oci::MANIFEST,
        &Manifest::artifact(BINARY_ARTIFACT, vec![layer]),
    )
    .await
}

pub async fn release(
    state: &AppState,
    harbor: &Harbor,
    project: &str,
    name: &str,
    version: &VersionRow,
    artifacts: &[ArtifactRow],
    signatures: &Signatures,
) -> Result<Released> {
    let repository = package_repository(project, name);

    let mut manifests = Vec::new();
    for artifact in artifacts {
        let target: Target = artifact.target.parse().map_err(ApiError::BadRequest)?;
        let mut descriptor =
            binary_manifest(state, harbor, &repository, &version.binary_name, artifact).await?;
        let (os, architecture) = target.platform();
        descriptor.artifact_type = Some(BINARY_ARTIFACT.to_owned());
        descriptor.platform = Some(Platform {
            architecture: architecture.to_owned(),
            os: os.to_owned(),
        });
        manifests.push(descriptor);
    }
    manifests.sort_by(|a, b| a.digest.cmp(&b.digest));

    let index = Index {
        schema_version: 2,
        media_type: oci::INDEX.to_owned(),
        artifact_type: Some(PACKAGE_ARTIFACT.to_owned()),
        manifests,
        annotations: BTreeMap::from([(
            "org.opencontainers.image.version".to_owned(),
            version.version.clone(),
        )]),
    };
    let body = serde_json::to_vec(&index).expect("indexes always serialise");
    let index_digest = oci::digest(&body);

    match harbor.oci().manifest(&repository, &version.version).await? {
        Some(existing) if existing.digest == index_digest => {}
        Some(existing) => {
            return Err(ApiError::Conflict(
                "tag_exists",
                format!(
                    "{repository}:{} already points at {}",
                    version.version, existing.digest
                ),
            ));
        }
        None => {
            harbor
                .oci()
                .put_manifest(&repository, &version.version, oci::INDEX, &body)
                .await?;
        }
    }

    cosign::sign(
        harbor,
        &state.signer,
        cosign_registry(state),
        &repository,
        &index_digest,
    )
    .await?;

    let signatures_body = serde_json::to_vec(signatures).expect("signatures always serialise");
    let layer = harbor
        .oci()
        .push_bytes(&repository, SIGNATURES_MEDIA_TYPE, &signatures_body)
        .await?;
    let mut manifest = Manifest::artifact(SIGNATURES_ARTIFACT, vec![layer]);
    manifest.subject = Some(Descriptor::new(
        oci::INDEX,
        index_digest.clone(),
        body.len() as u64,
    ));
    let signatures = put_by_digest(harbor, &repository, oci::MANIFEST, &manifest).await?;

    Ok(Released {
        index: index_digest,
        signatures: signatures.digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(media_type: &str, digest: &str, size: u64) -> Manifest {
        Manifest::artifact(
            BINARY_ARTIFACT,
            vec![Descriptor::new(media_type, digest.to_owned(), size)],
        )
    }

    #[test]
    fn binary_manifests_are_validated() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let ok = manifest("application/octet-stream", &digest, 10);
        assert!(validate_binary_manifest(&ok, Target::LinuxAmd64, 100).is_ok());
        assert!(validate_binary_manifest(&ok, Target::Wasm, 100).is_err());
        assert!(validate_binary_manifest(&ok, Target::LinuxAmd64, 5).is_err());

        let upper = manifest(
            "application/octet-stream",
            &digest.to_uppercase().replace("SHA256", "sha256"),
            10,
        );
        assert!(validate_binary_manifest(&upper, Target::LinuxAmd64, 100).is_err());

        let mut wrong_type = ok.clone();
        wrong_type.artifact_type = Some("application/vnd.other".to_owned());
        assert!(validate_binary_manifest(&wrong_type, Target::LinuxAmd64, 100).is_err());

        let mut two = ok.clone();
        two.layers.push(two.layers[0].clone());
        assert!(validate_binary_manifest(&two, Target::LinuxAmd64, 100).is_err());
    }
}
