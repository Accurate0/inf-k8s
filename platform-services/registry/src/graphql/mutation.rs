use async_graphql::{Context, ID, InputObject, Object, Result, SimpleObject};
use axum::Json;
use axum::extract::{Path, State};
use registry_api::signing::{CertificateRequest, PublishRequest};
use registry_api::{
    CreatePackage, CreateToken, CreateVersion, ECOSYSTEM_CLI, HarborSignRequest,
    NewTrustedPublisher, RegisterArtifact, UpdatePackage, Yank,
};

use super::objects::{
    Artifact, Package, PublishToken, RegistryTarget, TrustedPublisher, Version, split_version_key,
};
use super::publishing::{
    Access, CreateVersionInput, HarborCredential, ImageSignature, SignedEnvelope,
};
use super::query::uuid;
use super::{Gql, IDS, principal, state};
use crate::cli::{artifacts, packages, versions};
use crate::core::{exchange, publishers, signing, tokens};

pub struct MutationRoot;

#[derive(InputObject)]
#[graphql(name = "RegistryCreatePackageInput")]
pub struct CreatePackageInput {
    name: String,
    #[graphql(default)]
    description: String,
    binary_name: Option<String>,
    #[graphql(default)]
    aliases: Vec<String>,
}

#[derive(InputObject)]
#[graphql(name = "RegistryUpdatePackageInput")]
pub struct UpdatePackageInput {
    description: Option<String>,
    binary_name: Option<String>,
    aliases: Option<Vec<String>>,
}

#[derive(InputObject)]
#[graphql(name = "RegistryCreatePublishTokenInput")]
pub struct CreatePublishTokenInput {
    name: String,
    /// CLI package names the token may publish.
    packages: Vec<String>,
    expires_in_days: Option<i64>,
}

#[derive(InputObject)]
#[graphql(name = "RegistryCreateTrustedPublisherInput")]
pub struct CreateTrustedPublisherInput {
    package: String,
    /// `owner/repo` on Forgejo.
    repository: String,
    ref_pattern: Option<String>,
    workflow: Option<String>,
}

#[derive(SimpleObject)]
#[graphql(name = "RegistryCreatedPublishToken")]
pub struct CreatedPublishToken {
    /// Shown once; only its hash is stored.
    secret: String,
    token: PublishToken,
}

#[Object(name = "Mutation")]
impl MutationRoot {
    async fn registry_create_package(
        &self,
        ctx: &Context<'_>,
        input: CreatePackageInput,
    ) -> Result<Package> {
        let (_, package) = packages::create(
            State(state(ctx).clone()),
            principal(ctx)?,
            Json(CreatePackage {
                name: input.name,
                description: input.description,
                binary_name: input.binary_name,
                aliases: input.aliases,
            }),
        )
        .await
        .gql()?;
        Ok(Package(package.0))
    }

    async fn registry_update_package(
        &self,
        ctx: &Context<'_>,
        name: String,
        input: UpdatePackageInput,
    ) -> Result<Package> {
        let package = packages::update(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path(name),
            Json(UpdatePackage {
                description: input.description,
                binary_name: input.binary_name,
                aliases: input.aliases,
            }),
        )
        .await
        .gql()?;
        Ok(Package(package.0))
    }

    async fn registry_archive_package(&self, ctx: &Context<'_>, name: String) -> Result<Package> {
        let package = packages::archive(State(state(ctx).clone()), principal(ctx)?, Path(name))
            .await
            .gql()?;
        Ok(Package(package.0))
    }

    async fn registry_unarchive_package(&self, ctx: &Context<'_>, name: String) -> Result<Package> {
        let package = packages::unarchive(State(state(ctx).clone()), principal(ctx)?, Path(name))
            .await
            .gql()?;
        Ok(Package(package.0))
    }

    /// Only packages without published versions can be deleted; archive the rest.
    async fn registry_delete_package(&self, ctx: &Context<'_>, name: String) -> Result<ID> {
        packages::delete(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path(name.clone()),
        )
        .await
        .gql()?;
        Ok(IDS.encode("package", name))
    }

    async fn registry_yank_version(
        &self,
        ctx: &Context<'_>,
        id: ID,
        reason: Option<String>,
    ) -> Result<Version> {
        let key = IDS.decode_as(&id, "version")?;
        let (package, version) = split_version_key(&key)?;
        let version = versions::yank(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path((package.to_owned(), version.to_owned())),
            Some(Json(Yank { reason })),
        )
        .await
        .gql()?;
        Ok(Version(version.0))
    }

    async fn registry_unyank_version(&self, ctx: &Context<'_>, id: ID) -> Result<Version> {
        let key = IDS.decode_as(&id, "version")?;
        let (package, version) = split_version_key(&key)?;
        let version = versions::unyank(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path((package.to_owned(), version.to_owned())),
        )
        .await
        .gql()?;
        Ok(Version(version.0))
    }

    async fn registry_create_publish_token(
        &self,
        ctx: &Context<'_>,
        input: CreatePublishTokenInput,
    ) -> Result<CreatedPublishToken> {
        let (_, created) = tokens::create(
            State(state(ctx).clone()),
            principal(ctx)?,
            Json(CreateToken {
                name: input.name,
                scopes: input
                    .packages
                    .into_iter()
                    .map(|package| registry_api::Scope {
                        ecosystem: ECOSYSTEM_CLI.to_owned(),
                        package,
                    })
                    .collect(),
                expires_in_days: input.expires_in_days,
            }),
        )
        .await
        .gql()?;

        Ok(CreatedPublishToken {
            token: PublishToken::load(ctx, created.0.id).await?,
            secret: created.0.token,
        })
    }

    async fn registry_revoke_publish_token(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<PublishToken> {
        let id = uuid(&IDS.decode_as(&id, "publishToken")?)?;
        tokens::revoke(State(state(ctx).clone()), principal(ctx)?, Path(id))
            .await
            .gql()?;
        PublishToken::load(ctx, id).await
    }

    async fn registry_create_trusted_publisher(
        &self,
        ctx: &Context<'_>,
        input: CreateTrustedPublisherInput,
    ) -> Result<TrustedPublisher> {
        let (_, publisher) = publishers::create(
            State(state(ctx).clone()),
            principal(ctx)?,
            Json(NewTrustedPublisher {
                ecosystem: ECOSYSTEM_CLI.to_owned(),
                package: input.package,
                repository: input.repository,
                ref_pattern: input.ref_pattern,
                workflow: input.workflow,
            }),
        )
        .await
        .gql()?;
        Ok(TrustedPublisher(publisher.0))
    }

    async fn registry_delete_trusted_publisher(&self, ctx: &Context<'_>, id: ID) -> Result<ID> {
        let uuid = uuid(&IDS.decode_as(&id, "trustedPublisher")?)?;
        publishers::delete(State(state(ctx).clone()), principal(ctx)?, Path(uuid))
            .await
            .gql()?;
        Ok(id)
    }

    /// Starts a draft version; push each target to harbor, register it, then publish.
    async fn registry_create_version(
        &self,
        ctx: &Context<'_>,
        input: CreateVersionInput,
    ) -> Result<Version> {
        let (_, version) = versions::create(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path(input.package),
            Json(CreateVersion {
                version: input.version,
                targets: input.targets.into_iter().map(Into::into).collect(),
                description: input.description,
                binary_name: input.binary_name,
                aliases: input.aliases,
                auth: input.auth.map(Into::into),
            }),
        )
        .await
        .gql()?;
        Ok(Version(version.0))
    }

    /// Records a target of a draft version already pushed to harbor as `manifest`.
    async fn registry_register_artifact(
        &self,
        ctx: &Context<'_>,
        package: String,
        version: String,
        target: RegistryTarget,
        manifest: String,
    ) -> Result<Artifact> {
        let target: registry_api::Target = target.into();
        let artifact = artifacts::register(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path((package.clone(), version.clone(), target.as_str().to_owned())),
            Json(RegisterArtifact { manifest }),
        )
        .await
        .gql()?;
        Ok(Artifact {
            package,
            version,
            artifact: artifact.0,
        })
    }

    /// Certifies a throwaway publish key for one package, after checking the caller may publish it.
    async fn registry_issue_signing_certificate(
        &self,
        ctx: &Context<'_>,
        package: String,
        public_key: String,
    ) -> Result<SignedEnvelope> {
        let certificate = signing::certificate(
            State(state(ctx).clone()),
            principal(ctx)?,
            Json(CertificateRequest {
                ecosystem: ECOSYSTEM_CLI.to_owned(),
                package,
                public_key,
            }),
        )
        .await
        .gql()?;
        Ok(certificate.0.into())
    }

    /// Publishes a draft whose targets are all registered, with its signed statement.
    async fn registry_publish_version(
        &self,
        ctx: &Context<'_>,
        package: String,
        version: String,
        statement: SignedEnvelope,
        certificate: SignedEnvelope,
    ) -> Result<Version> {
        let version = versions::publish(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path((package, version)),
            Json(PublishRequest {
                statement: statement.into(),
                certificate: certificate.into(),
            }),
        )
        .await
        .gql()?;
        Ok(Version(version.0))
    }

    /// Mints a short-lived harbor robot limited to `access`.
    async fn registry_create_harbor_credential(
        &self,
        ctx: &Context<'_>,
        access: Vec<Access>,
    ) -> Result<HarborCredential> {
        let token = exchange::issue(
            state(ctx),
            &principal(ctx)?,
            access.into_iter().map(Into::into).collect(),
        )
        .await
        .gql()?;
        Ok(token.into())
    }

    async fn registry_revoke_harbor_credential(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> Result<String> {
        exchange::revoke(
            State(state(ctx).clone()),
            principal(ctx)?,
            Path(username.clone()),
        )
        .await
        .gql()?;
        Ok(username)
    }

    /// Cosign-signs `repository@digest` in harbor with the registry key.
    async fn registry_sign_image(
        &self,
        ctx: &Context<'_>,
        repository: String,
        digest: String,
    ) -> Result<ImageSignature> {
        let signature = exchange::sign(
            State(state(ctx).clone()),
            principal(ctx)?,
            Json(HarborSignRequest { repository, digest }),
        )
        .await
        .gql()?;
        Ok(signature.0.into())
    }

    /// Admins: push every published version missing from harbor, in the background.
    async fn registry_admin_backfill(&self, ctx: &Context<'_>) -> Result<bool> {
        let _ = packages::backfill(State(state(ctx).clone()), principal(ctx)?)
            .await
            .gql()?;
        Ok(true)
    }

    /// Admins: re-render and sign the catalog now, returning its new seq.
    async fn registry_admin_render_catalog(&self, ctx: &Context<'_>) -> Result<i64> {
        let rendered = packages::render_catalog(State(state(ctx).clone()), principal(ctx)?)
            .await
            .gql()?;
        Ok(rendered.0["seq"].as_i64().unwrap_or_default())
    }
}
