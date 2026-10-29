use anyhow::{Context, bail};
use registry_api::{
    Artifact, Target, Version,
    signing::{self, PublicKeyHandle},
};

include!(concat!(env!("OUT_DIR"), "/trusted_keys.rs"));

fn roots() -> anyhow::Result<Vec<PublicKeyHandle>> {
    TRUSTED_KEYS
        .iter()
        .map(|pem| signing::verifying_key_from_pem(pem).map_err(anyhow::Error::msg))
        .collect()
}

pub fn verified_artifact(
    name: &str,
    version: &Version,
    target: Target,
) -> anyhow::Result<Artifact> {
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

    manifest
        .statement
        .artifacts
        .into_iter()
        .find(|a| a.target == target)
        .with_context(|| format!("{name}@{} has no signed {target} build", version.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_keys_parse() {
        assert!(!roots().unwrap().is_empty());
    }
}
