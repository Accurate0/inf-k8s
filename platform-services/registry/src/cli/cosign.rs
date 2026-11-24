use base64::{Engine, engine::general_purpose::STANDARD};
use p256::ecdsa::{Signature, signature::Signer as _};
use registry_api::oci::{self, Descriptor, Manifest};
use serde_json::{Value, json};

use crate::{auth::ForgejoClaims, error::Result, harbor::Harbor, state::Signer};

pub const SIMPLE_SIGNING: &str = "application/vnd.dev.cosign.simplesigning.v1+json";
pub const SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";
const IMAGE_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
pub const DSSE_ENVELOPE: &str = "application/vnd.dsse.envelope.v1+json";
pub const IN_TOTO: &str = "application/vnd.in-toto+json";
pub const SLSA_PROVENANCE: &str = "https://slsa.dev/provenance/v1";
const PREDICATE_ANNOTATION: &str = "predicateType";
const BUILD_TYPE: &str = "https://forgejo.org/actions/workflow@v1";

pub fn signature_tag(digest: &str) -> String {
    format!("{}.sig", digest.replacen(':', "-", 1))
}

pub fn attestation_tag(digest: &str) -> String {
    format!("{}.att", digest.replacen(':', "-", 1))
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

    push_manifest(harbor, repository, layer, &signature_tag(digest)).await
}

pub fn provenance(
    registry: &str,
    forge: &str,
    repository: &str,
    digest: &str,
    claims: &ForgejoClaims,
) -> Value {
    let forge = forge.trim_end_matches('/');
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    let builder = claims
        .workflow_ref
        .as_deref()
        .map(|workflow| format!("{forge}/{workflow}"))
        .unwrap_or_else(|| format!("{forge}/{}", claims.repository));

    let mut dependency = json!({ "uri": format!("git+{forge}/{}", claims.repository) });
    if let Some(git_ref) = &claims.git_ref {
        dependency["uri"] = json!(format!("git+{forge}/{}@{git_ref}", claims.repository));
    }
    if let Some(sha) = &claims.sha {
        dependency["digest"] = json!({ "gitCommit": sha });
    }

    let mut metadata = json!({});
    if let Some(run_id) = &claims.run_id {
        metadata["invocationId"] = json!(format!(
            "{forge}/{}/actions/runs/{run_id}",
            claims.repository
        ));
    }

    json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{
            "name": format!("{registry}/{repository}"),
            "digest": { "sha256": hex },
        }],
        "predicateType": SLSA_PROVENANCE,
        "predicate": {
            "buildDefinition": {
                "buildType": BUILD_TYPE,
                "externalParameters": {
                    "repository": claims.repository,
                    "ref": claims.git_ref,
                    "workflowRef": claims.workflow_ref,
                    "eventName": claims.event_name,
                },
                "internalParameters": {
                    "repositoryId": claims.repository_id,
                    "actor": claims.actor,
                },
                "resolvedDependencies": [dependency],
            },
            "runDetails": {
                "builder": { "id": builder },
                "metadata": metadata,
            },
        },
    })
}

pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "DSSEv1 {} {payload_type} {} ",
        payload_type.len(),
        payload.len()
    )
    .into_bytes();
    out.extend_from_slice(payload);
    out
}

pub fn envelope(signer: &Signer, statement: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(statement).expect("statements always serialise");
    let signature: Signature = signer.key().sign(&pae(IN_TOTO, &payload));

    serde_json::to_vec(&json!({
        "payloadType": IN_TOTO,
        "payload": STANDARD.encode(&payload),
        "signatures": [{ "keyid": "", "sig": STANDARD.encode(signature.to_der().as_bytes()) }],
    }))
    .expect("envelopes always serialise")
}

pub async fn attest(
    harbor: &Harbor,
    signer: &Signer,
    repository: &str,
    digest: &str,
    statement: &Value,
) -> Result<()> {
    let envelope = envelope(signer, statement);

    let mut layer = harbor
        .oci()
        .push_bytes(repository, DSSE_ENVELOPE, &envelope)
        .await?;
    layer
        .annotations
        .insert(SIGNATURE_ANNOTATION.to_owned(), String::new());
    layer
        .annotations
        .insert(PREDICATE_ANNOTATION.to_owned(), SLSA_PROVENANCE.to_owned());

    push_manifest(harbor, repository, layer, &attestation_tag(digest)).await
}

async fn push_manifest(
    harbor: &Harbor,
    repository: &str,
    layer: Descriptor,
    tag: &str,
) -> Result<()> {
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
        .put_manifest(repository, tag, oci::MANIFEST, &body)
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
    fn attestations_follow_the_cosign_scheme() {
        assert_eq!(
            attestation_tag(&format!("sha256:{}", "ab".repeat(32))),
            format!("sha256-{}.att", "ab".repeat(32))
        );
    }

    #[test]
    fn pae_matches_the_dsse_spec() {
        assert_eq!(
            pae("http://example.com/HelloWorld", b"hello world"),
            b"DSSEv1 29 http://example.com/HelloWorld 11 hello world".to_vec()
        );
    }

    #[test]
    fn provenance_describes_the_forgejo_run() {
        let claims = ForgejoClaims {
            repository: "anurag/k8s".to_owned(),
            git_ref: Some("refs/heads/main".to_owned()),
            sha: Some("deadbeef".to_owned()),
            run_id: Some("7".to_owned()),
            workflow_ref: Some(
                "anurag/k8s/.forgejo/workflows/app-x.yaml@refs/heads/main".to_owned(),
            ),
            ..Default::default()
        };
        let statement = provenance(
            "registry.anurag.sh",
            "https://git.anurag.sh/",
            "containers/app",
            &format!("sha256:{}", "ab".repeat(32)),
            &claims,
        );

        assert_eq!(statement["predicateType"], SLSA_PROVENANCE);
        assert_eq!(
            statement["subject"][0]["name"],
            "registry.anurag.sh/containers/app"
        );
        assert_eq!(statement["subject"][0]["digest"]["sha256"], "ab".repeat(32));

        let predicate = &statement["predicate"];
        assert_eq!(
            predicate["runDetails"]["builder"]["id"],
            "https://git.anurag.sh/anurag/k8s/.forgejo/workflows/app-x.yaml@refs/heads/main"
        );
        assert_eq!(
            predicate["runDetails"]["metadata"]["invocationId"],
            "https://git.anurag.sh/anurag/k8s/actions/runs/7"
        );
        assert_eq!(
            predicate["buildDefinition"]["resolvedDependencies"][0],
            json!({
                "uri": "git+https://git.anurag.sh/anurag/k8s@refs/heads/main",
                "digest": { "gitCommit": "deadbeef" },
            })
        );
    }

    #[test]
    fn envelopes_verify_with_the_signer() {
        use p256::ecdsa::signature::Verifier;

        let signer = Signer::new(registry_api::signing::generate());
        let statement = json!({ "_type": "https://in-toto.io/Statement/v1" });
        let envelope: Value = serde_json::from_slice(&envelope(&signer, &statement)).unwrap();

        assert_eq!(envelope["payloadType"], IN_TOTO);
        let payload = STANDARD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&payload).unwrap(),
            statement
        );

        let sig = STANDARD
            .decode(envelope["signatures"][0]["sig"].as_str().unwrap())
            .unwrap();
        signer
            .verifying_key()
            .verify(&pae(IN_TOTO, &payload), &Signature::from_der(&sig).unwrap())
            .unwrap();
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
