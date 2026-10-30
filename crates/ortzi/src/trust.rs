use anyhow::{Context, bail};
use registry_api::{
    Artifact, Completions, Permissions, Target, Version,
    signing::{self, PublicKeyHandle, Statement},
};

include!(concat!(env!("OUT_DIR"), "/trusted_keys.rs"));

fn roots() -> anyhow::Result<Vec<PublicKeyHandle>> {
    TRUSTED_KEYS
        .iter()
        .map(|pem| signing::verifying_key_from_pem(pem).map_err(anyhow::Error::msg))
        .collect()
}

pub struct Verified {
    pub artifact: Artifact,
    pub permissions: Option<Permissions>,
    pub completions: Option<Completions>,
}

pub fn verified_artifact(name: &str, version: &Version, host: Target) -> anyhow::Result<Verified> {
    let statement = verified_statement(name, version)?;
    let artifact = pick(&statement.artifacts, host).cloned().with_context(|| {
        format!(
            "{name}@{} has no signed {host} or wasm build",
            version.version
        )
    })?;
    Ok(Verified {
        artifact,
        permissions: statement.permissions,
        completions: statement.completions,
    })
}

fn pick(artifacts: &[Artifact], host: Target) -> Option<&Artifact> {
    artifacts
        .iter()
        .find(|a| a.target == host)
        .or_else(|| artifacts.iter().find(|a| a.target == Target::Wasm))
}

pub fn verified_statement(name: &str, version: &Version) -> anyhow::Result<Statement> {
    let Some(signatures) = &version.signatures else {
        bail!(
            "{name}@{} is not signed; refusing to install",
            version.version
        );
    };
    let manifest =
        signing::verify_release(&roots()?, name, &version.version, signatures).map_err(|e| {
            anyhow::anyhow!(
                "{name}@{} failed signature verification: {e}",
                version.version
            )
        })?;

    Ok(manifest.statement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_native_artifacts() {
        let artifact = |target| Artifact {
            target,
            sha256: String::new(),
            size: 0,
        };
        let both = [artifact(Target::Wasm), artifact(Target::DarwinArm64)];
        assert_eq!(
            pick(&both, Target::DarwinArm64).unwrap().target,
            Target::DarwinArm64
        );
        assert_eq!(
            pick(&both, Target::LinuxAmd64).unwrap().target,
            Target::Wasm
        );
        assert!(pick(&[artifact(Target::LinuxAmd64)], Target::DarwinArm64).is_none());
    }

    #[test]
    fn trusted_keys_parse() {
        assert!(!roots().unwrap().is_empty());
    }
}
