use async_graphql::{Context, ID, Object, Result};
use uuid::Uuid;

use super::objects::{
    Artifact, AuditEntry, Package, PublishToken, SigningKey, TrustedPublisher, Version, WhoAmI,
    split_version_key,
};
use super::{Gql, IDS, principal};
use crate::error::ApiError;

pub struct QueryRoot;

#[Object(name = "Query")]
impl QueryRoot {
    async fn registry_whoami(&self, ctx: &Context<'_>) -> Result<WhoAmI> {
        let principal = principal(ctx)?;
        let groups = match &principal {
            crate::auth::Principal::User { groups, .. } => groups.clone(),
            _ => Vec::new(),
        };
        Ok(WhoAmI {
            subject: principal.actor(),
            admin: principal.is_admin(),
            groups,
        })
    }

    /// Unarchived packages, optionally filtered by name or description.
    async fn registry_packages(
        &self,
        ctx: &Context<'_>,
        query: Option<String>,
    ) -> Result<Vec<Package>> {
        Package::list(ctx, query.filter(|q| !q.is_empty())).await
    }

    async fn registry_package(&self, ctx: &Context<'_>, name: String) -> Result<Package> {
        Package::load(ctx, &name).await
    }

    async fn registry_version(
        &self,
        ctx: &Context<'_>,
        package: String,
        version: String,
    ) -> Result<Version> {
        Version::load(ctx, &package, &version).await
    }

    async fn registry_publish_tokens(&self, ctx: &Context<'_>) -> Result<Vec<PublishToken>> {
        PublishToken::list(ctx).await
    }

    async fn registry_trusted_publishers(
        &self,
        ctx: &Context<'_>,
        package: Option<String>,
    ) -> Result<Vec<TrustedPublisher>> {
        TrustedPublisher::list(ctx, package).await
    }

    async fn registry_audit(
        &self,
        ctx: &Context<'_>,
        package: Option<String>,
        #[graphql(default = 100)] limit: i64,
    ) -> Result<Vec<AuditEntry>> {
        AuditEntry::list(ctx, package, limit).await
    }

    async fn registry_signing_keys(&self, ctx: &Context<'_>) -> Vec<SigningKey> {
        SigningKey::list(ctx).await
    }

    #[graphql(entity)]
    async fn find_package_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Package> {
        Package::load(ctx, &IDS.decode_as(&id, "package")?).await
    }

    #[graphql(entity)]
    async fn find_version_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Version> {
        let key = IDS.decode_as(&id, "version")?;
        let (package, version) = split_version_key(&key)?;
        Version::load(ctx, package, version).await
    }

    #[graphql(entity)]
    async fn find_artifact_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Artifact> {
        Artifact::load(ctx, &IDS.decode_as(&id, "artifact")?).await
    }

    #[graphql(entity)]
    async fn find_publish_token_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<PublishToken> {
        PublishToken::load(ctx, uuid(&IDS.decode_as(&id, "publishToken")?)?).await
    }

    #[graphql(entity)]
    async fn find_trusted_publisher_by_id(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<TrustedPublisher> {
        TrustedPublisher::load(ctx, uuid(&IDS.decode_as(&id, "trustedPublisher")?)?).await
    }

    #[graphql(entity)]
    async fn find_audit_entry_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<AuditEntry> {
        let row_id = IDS.decode_row_id(&id, "auditEntry")?;
        principal(ctx)?.require_admin().gql()?;

        let entry = sqlx::query_as!(
            registry_api::AuditEntry,
            "SELECT id, at, actor, actor_kind, action, ecosystem, package, version, detail
             FROM core_audit_log WHERE id = $1",
            row_id
        )
        .fetch_optional(&super::state(ctx).pool)
        .await
        .map_err(ApiError::from)
        .gql()?
        .ok_or_else(|| async_graphql::Error::new(format!("audit entry {row_id} not found")))?;
        Ok(entry.into())
    }

    #[graphql(entity)]
    async fn find_signing_key_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<SigningKey> {
        let key_id = IDS.decode_as(&id, "signingKey")?;
        SigningKey::list(ctx)
            .await
            .into_iter()
            .find(|key| key.key_id == key_id)
            .ok_or_else(|| async_graphql::Error::new(format!("signing key {key_id} not found")))
    }
}

pub fn uuid(key: &str) -> Result<Uuid> {
    key.parse()
        .map_err(|_| async_graphql::Error::new(format!("invalid id {key:?}")))
}
