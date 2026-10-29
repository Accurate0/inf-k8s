use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use p256::{
    ecdsa::{
        Signature, SigningKey, VerifyingKey,
        signature::{Signer, Verifier},
    },
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePublicKey, LineEnding},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{Artifact, Permissions, Source};

pub use p256::ecdsa::{SigningKey as PrivateKey, VerifyingKey as PublicKeyHandle};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub package: String,
    pub version: String,
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateClaims {
    pub ecosystem: String,
    pub package: String,
    pub public_key: String,
    pub subject: String,
    pub kind: String,
    pub source: Option<Source>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestClaims {
    pub statement: Statement,
    pub signed_by: String,
    pub published_by: String,
    pub source: Option<Source>,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signatures {
    pub manifest: Envelope,
    pub certificate: Envelope,
    pub statement: Envelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateRequest {
    pub ecosystem: String,
    pub package: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    pub statement: Envelope,
    pub certificate: Envelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKey {
    pub key_id: String,
    pub public_key_pem: String,
}

pub fn generate() -> SigningKey {
    use rand::Rng;
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Ok(key) = SigningKey::from_slice(&bytes) {
            return key;
        }
    }
}

pub fn signing_key_from_pem(pem: &str) -> Result<SigningKey, String> {
    SigningKey::from_pkcs8_pem(pem.trim()).map_err(|e| format!("invalid signing key: {e}"))
}

pub fn verifying_key_from_pem(pem: &str) -> Result<VerifyingKey, String> {
    VerifyingKey::from_public_key_pem(pem.trim()).map_err(|e| format!("invalid public key: {e}"))
}

pub fn public_key_pem(key: &VerifyingKey) -> String {
    key.to_public_key_pem(LineEnding::LF)
        .expect("p256 keys always encode")
}

pub fn key_id(key: &VerifyingKey) -> String {
    let der = key.to_public_key_der().expect("p256 keys always encode");
    hex::encode(&Sha256::digest(der.as_bytes())[..8])
}

pub fn encode_key(key: &VerifyingKey) -> String {
    STANDARD.encode(key.to_encoded_point(true).as_bytes())
}

pub fn decode_key(encoded: &str) -> Result<VerifyingKey, String> {
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|e| format!("invalid key encoding: {e}"))?;
    VerifyingKey::from_sec1_bytes(&bytes).map_err(|e| format!("invalid key: {e}"))
}

pub fn sign<T: Serialize>(key: &SigningKey, value: &T) -> Envelope {
    let payload = serde_json::to_vec(value).expect("signed values always serialise");
    let signature: Signature = key.sign(&payload);
    Envelope {
        payload: STANDARD.encode(&payload),
        signature: STANDARD.encode(signature.to_bytes()),
        key_id: key_id(key.verifying_key()),
    }
}

pub fn open<T: DeserializeOwned>(envelope: &Envelope, key: &VerifyingKey) -> Result<T, String> {
    if envelope.key_id != key_id(key) {
        return Err(format!(
            "signed by key {} but expected {}",
            envelope.key_id,
            key_id(key)
        ));
    }
    let payload = STANDARD
        .decode(&envelope.payload)
        .map_err(|e| format!("invalid payload encoding: {e}"))?;
    let signature = STANDARD
        .decode(&envelope.signature)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or("invalid signature encoding")?;
    key.verify(&payload, &signature)
        .map_err(|_| "signature does not verify".to_owned())?;
    serde_json::from_slice(&payload).map_err(|e| format!("invalid signed payload: {e}"))
}

pub fn root_for<'a>(
    roots: &'a [VerifyingKey],
    envelope: &Envelope,
) -> Result<&'a VerifyingKey, String> {
    roots
        .iter()
        .find(|key| key_id(key) == envelope.key_id)
        .ok_or_else(|| format!("signed by untrusted key {}", envelope.key_id))
}

pub fn verify_certificate(
    roots: &[VerifyingKey],
    certificate: &Envelope,
    ecosystem: &str,
    package: &str,
) -> Result<(CertificateClaims, VerifyingKey), String> {
    let claims: CertificateClaims = open(certificate, root_for(roots, certificate)?)?;
    if claims.ecosystem != ecosystem || claims.package != package {
        return Err(format!(
            "certificate is for {}/{}, not {ecosystem}/{package}",
            claims.ecosystem, claims.package
        ));
    }
    let key = decode_key(&claims.public_key)?;
    Ok((claims, key))
}

pub fn verify_release(
    roots: &[VerifyingKey],
    package: &str,
    version: &str,
    signatures: &Signatures,
) -> Result<ManifestClaims, String> {
    let manifest: ManifestClaims =
        open(&signatures.manifest, root_for(roots, &signatures.manifest)?)?;
    if manifest.statement.package != package || manifest.statement.version != version {
        return Err(format!(
            "manifest is for {}@{}, not {package}@{version}",
            manifest.statement.package, manifest.statement.version
        ));
    }

    let (certificate, key) = verify_certificate(
        roots,
        &signatures.certificate,
        crate::ECOSYSTEM_CLI,
        package,
    )?;
    if manifest.published_at < certificate.issued_at
        || manifest.published_at > certificate.expires_at
    {
        return Err("published outside the signing certificate's validity".to_owned());
    }
    if manifest.signed_by != certificate.subject {
        return Err("manifest and certificate disagree on the signer".to_owned());
    }

    let statement: Statement = open(&signatures.statement, &key)?;
    if statement != manifest.statement {
        return Err("publisher statement does not match the registry manifest".to_owned());
    }

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Target;

    fn statement() -> Statement {
        Statement {
            package: "ff".to_owned(),
            version: "1.0.0".to_owned(),
            artifacts: vec![Artifact {
                target: Target::LinuxAmd64,
                sha256: "ab".repeat(32),
                size: 10,
            }],
            permissions: None,
        }
    }

    fn release(root: &SigningKey, signer: &SigningKey, package: &str) -> Signatures {
        let now = Utc::now();
        let certificate = sign(
            root,
            &CertificateClaims {
                ecosystem: "cli".to_owned(),
                package: package.to_owned(),
                public_key: encode_key(signer.verifying_key()),
                subject: "forgejo:anurag/k8s".to_owned(),
                kind: "forgejo".to_owned(),
                source: None,
                issued_at: now,
                expires_at: now + chrono::Duration::minutes(10),
            },
        );
        let manifest = sign(
            root,
            &ManifestClaims {
                statement: statement(),
                signed_by: "forgejo:anurag/k8s".to_owned(),
                published_by: "forgejo:anurag/k8s".to_owned(),
                source: None,
                published_at: now + chrono::Duration::seconds(5),
            },
        );
        Signatures {
            manifest,
            certificate,
            statement: sign(signer, &statement()),
        }
    }

    #[test]
    fn a_full_release_verifies() {
        let (root, signer) = (generate(), generate());
        let roots = [*root.verifying_key()];

        let manifest =
            verify_release(&roots, "ff", "1.0.0", &release(&root, &signer, "ff")).unwrap();
        assert_eq!(manifest.statement, statement());
    }

    #[test]
    fn tampering_is_detected() {
        let (root, signer) = (generate(), generate());
        let roots = [*root.verifying_key()];

        assert!(
            verify_release(
                &[*generate().verifying_key()],
                "ff",
                "1.0.0",
                &release(&root, &signer, "ff")
            )
            .is_err()
        );
        assert!(verify_release(&roots, "ai", "1.0.0", &release(&root, &signer, "ff")).is_err());
        assert!(verify_release(&roots, "ff", "1.0.0", &release(&root, &signer, "ai")).is_err());

        let mut forged = release(&root, &signer, "ff");
        let mut evil = statement();
        evil.artifacts[0].sha256 = "cd".repeat(32);
        forged.statement = sign(&generate(), &evil);
        assert!(verify_release(&roots, "ff", "1.0.0", &forged).is_err());

        let mut swapped = release(&root, &signer, "ff");
        swapped.manifest.payload = sign(
            &root,
            &ManifestClaims {
                statement: evil,
                signed_by: "forgejo:anurag/k8s".to_owned(),
                published_by: "x".to_owned(),
                source: None,
                published_at: Utc::now(),
            },
        )
        .payload;
        assert!(verify_release(&roots, "ff", "1.0.0", &swapped).is_err());
    }

    #[test]
    fn keys_round_trip() {
        let key = generate();
        let pem = public_key_pem(key.verifying_key());
        assert_eq!(verifying_key_from_pem(&pem).unwrap(), *key.verifying_key());
        assert_eq!(
            decode_key(&encode_key(key.verifying_key())).unwrap(),
            *key.verifying_key()
        );
    }
}
