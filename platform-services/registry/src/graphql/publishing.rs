use async_graphql::{Enum, InputObject, SimpleObject};
use chrono::{DateTime, Utc};
use registry_api::{HarborAccess, HarborAction, PackageAuth, signing::Envelope};

use super::objects::RegistryTarget;

#[derive(InputObject)]
#[graphql(name = "RegistryPackageAuthInput")]
pub struct PackageAuthInput {
    pub issuer: String,
    pub client_id: String,
    pub scopes: String,
    pub token_env: String,
    #[graphql(default)]
    pub lazy: bool,
}

impl From<PackageAuthInput> for PackageAuth {
    fn from(input: PackageAuthInput) -> Self {
        Self {
            issuer: input.issuer,
            client_id: input.client_id,
            scopes: input.scopes,
            token_env: input.token_env,
            lazy: input.lazy,
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "RegistryCreateVersionInput")]
pub struct CreateVersionInput {
    pub package: String,
    pub version: String,
    pub targets: Vec<RegistryTarget>,
    pub description: Option<String>,
    pub binary_name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub auth: Option<PackageAuthInput>,
}

/// A signed payload; payload and signature are base64, exactly as signed.
#[derive(InputObject, SimpleObject)]
#[graphql(name = "RegistryEnvelope", input_name = "RegistryEnvelopeInput")]
pub struct SignedEnvelope {
    pub payload: String,
    pub signature: String,
    pub key_id: String,
}

impl From<SignedEnvelope> for Envelope {
    fn from(envelope: SignedEnvelope) -> Self {
        Self {
            payload: envelope.payload,
            signature: envelope.signature,
            key_id: envelope.key_id,
        }
    }
}

impl From<Envelope> for SignedEnvelope {
    fn from(envelope: Envelope) -> Self {
        Self {
            payload: envelope.payload,
            signature: envelope.signature,
            key_id: envelope.key_id,
        }
    }
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "RegistryHarborAction")]
pub enum RegistryHarborAction {
    Pull,
    Push,
}

impl From<RegistryHarborAction> for HarborAction {
    fn from(action: RegistryHarborAction) -> Self {
        match action {
            RegistryHarborAction::Pull => Self::Pull,
            RegistryHarborAction::Push => Self::Push,
        }
    }
}

impl From<HarborAction> for RegistryHarborAction {
    fn from(action: HarborAction) -> Self {
        match action {
            HarborAction::Pull => Self::Pull,
            HarborAction::Push => Self::Push,
        }
    }
}

#[derive(InputObject, SimpleObject)]
#[graphql(
    name = "RegistryHarborAccess",
    input_name = "RegistryHarborAccessInput"
)]
pub struct Access {
    pub project: String,
    pub actions: Vec<RegistryHarborAction>,
    /// The ortzi package a push to a package project is for.
    pub package: Option<String>,
}

impl From<Access> for HarborAccess {
    fn from(access: Access) -> Self {
        Self {
            project: access.project,
            actions: access.actions.into_iter().map(Into::into).collect(),
            package: access.package,
        }
    }
}

impl From<HarborAccess> for Access {
    fn from(access: HarborAccess) -> Self {
        Self {
            project: access.project,
            actions: access.actions.into_iter().map(Into::into).collect(),
            package: access.package,
        }
    }
}

/// A short-lived harbor robot; revoke it when done.
#[derive(SimpleObject)]
#[graphql(name = "RegistryHarborCredential")]
pub struct HarborCredential {
    pub registry: String,
    pub username: String,
    pub password: String,
    pub expires_at: DateTime<Utc>,
    pub access: Vec<Access>,
}

impl From<registry_api::HarborToken> for HarborCredential {
    fn from(token: registry_api::HarborToken) -> Self {
        Self {
            registry: token.registry,
            username: token.username,
            password: token.password,
            expires_at: token.expires_at,
            access: token.access.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryImageSignature")]
pub struct ImageSignature {
    pub repository: String,
    pub digest: String,
    pub tag: String,
    /// Tag of the SLSA provenance attestation, when one was attached.
    pub attestation: Option<String>,
}

impl From<registry_api::HarborSignature> for ImageSignature {
    fn from(signature: registry_api::HarborSignature) -> Self {
        Self {
            repository: signature.repository,
            digest: signature.digest,
            tag: signature.tag,
            attestation: signature.attestation,
        }
    }
}

/// The signed package catalog in harbor that installs resolve from.
#[derive(SimpleObject)]
#[graphql(name = "RegistryCatalogRef")]
pub struct CatalogRef {
    pub seq: i64,
    pub digest: Option<String>,
}
