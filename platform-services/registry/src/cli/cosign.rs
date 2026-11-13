use base64::{Engine, engine::general_purpose::STANDARD};
use p256::ecdsa::{Signature, signature::Signer as _};
use registry_api::oci::{self, Descriptor, Manifest};
use serde_json::json;

use crate::{error::Result, harbor::Harbor, state::Signer};

pub const SIMPLE_SIGNING: &str = "application/vnd.dev.cosign.simplesigning.v1+json";
pub const SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";
const IMAGE_CONFIG: &str = "application/vnd.oci.image.config.v1+json";

pub fn signature_tag(digest: &str) -> String {
    format!("{}.sig", digest.replacen(':', "-", 1))
}

pub fn payload(registry: &str, repository: &str, digest: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "critical": {
            "identity": { "docker-reference": format!("{registry}/{repository}") },
            "image": { "docker-manifest-digest": digest },
            "type": "cosign container image signature",
        },
        "optional": null,
    }))
    .expect("payloads always serialise")
}

pub async fn sign(
    harbor: &Harbor,
    signer: &Signer,
    registry: &str,
    repository: &str,
    digest: &str,
) -> Result<()> {
    let payload = payload(registry, repository, digest);
    let signature: Signature = signer.key().sign(&payload);

    let mut layer = harbor
        .oci()
        .push_bytes(repository, SIMPLE_SIGNING, &payload)
        .await?;
    layer.annotations.insert(
        SIGNATURE_ANNOTATION.to_owned(),
        STANDARD.encode(signature.to_der().as_bytes()),
    );

    let config = serde_json::to_vec(&json!({
        "architecture": "",
        "created": "0001-01-01T00:00:00Z",
        "history": [{ "created": "0001-01-01T00:00:00Z" }],
        "os": "",
        "rootfs": { "type": "layers", "diff_ids": [layer.digest] },
        "config": {},
    }))
    .expect("configs always serialise");
    let config = harbor
        .oci()
        .push_bytes(repository, IMAGE_CONFIG, &config)
        .await?;

    let manifest = Manifest {
        schema_version: 2,
        media_type: oci::MANIFEST.to_owned(),
        artifact_type: None,
        config: Descriptor::new(IMAGE_CONFIG, config.digest, config.size),
        layers: vec![layer],
        subject: None,
        annotations: Default::default(),
    };
    let body = serde_json::to_vec(&manifest).expect("manifests always serialise");
    harbor
        .oci()
        .put_manifest(repository, &signature_tag(digest), oci::MANIFEST, &body)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_follow_the_cosign_scheme() {
        assert_eq!(
            signature_tag(&format!("sha256:{}", "ab".repeat(32))),
            format!("sha256-{}.sig", "ab".repeat(32))
        );
    }

    #[test]
    fn payload_names_the_image() {
        let payload: serde_json::Value = serde_json::from_slice(&payload(
            "registry.anurag.sh",
            "ortzi-packages/ff",
            "sha256:x",
        ))
        .unwrap();
        assert_eq!(
            payload["critical"]["identity"]["docker-reference"],
            "registry.anurag.sh/ortzi-packages/ff"
        );
        assert_eq!(
            payload["critical"]["image"]["docker-manifest-digest"],
            "sha256:x"
        );
        assert_eq!(
            payload["critical"]["type"],
            "cosign container image signature"
        );
    }
}
