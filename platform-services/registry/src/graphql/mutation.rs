use async_graphql::{Context, ID, InputObject, Object, Result, SimpleObject};
use axum::Json;
use axum::extract::{Path, State};
use registry_api::{
    CreatePackage, CreateToken, ECOSYSTEM_CLI, NewTrustedPublisher, PromoteRequest, UpdatePackage,
    Yank,
};

use super::objects::{
    Package, Promoted, PublishToken, TrustedPublisher, Version, split_version_key,
};
use super::query::uuid;
use super::{Gql, IDS, deployer, principal, state};
use crate::cli::{packages, versions};
use crate::core::{publishers, tokens};

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

#[derive(InputObject)]
#[graphql(name = "RegistryPromoteInput")]
pub struct PromoteInput {
    /// The app.yaml name the image deploys.
    app: String,
    /// `<project>/<name>` in harbor, e.g. `containers/spotify`.
    repository: String,
    /// A digest, or a tag such as the commit sha.
    reference: String,
    /// Allow promoting a build older than the deployed one, i.e. a rollback.
    #[graphql(default)]
    force: bool,
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

    /// Point the repository's deploy tag at a signed build and publish the apps bundle.
    async fn registry_promote(&self, ctx: &Context<'_>, input: PromoteInput) -> Result<Promoted> {
        let principal = deployer(ctx).await?;
        let promoted = crate::core::promote::promote(
            state(ctx),
            &principal,
            PromoteRequest {
                app: input.app,
                repository: input.repository,
                reference: input.reference,
                force: input.force,
            },
        )
        .await
        .gql()?;
        Ok(Promoted {
            promotion: promoted.promotion.into(),
            previous: promoted.previous,
            changed: promoted.changed,
        })
    }

    async fn registry_delete_trusted_publisher(&self, ctx: &Context<'_>, id: ID) -> Result<ID> {
        let uuid = uuid(&IDS.decode_as(&id, "trustedPublisher")?)?;
        publishers::delete(State(state(ctx).clone()), principal(ctx)?, Path(uuid))
            .await
            .gql()?;
        Ok(id)
    }
}
