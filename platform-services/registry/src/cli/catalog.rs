use std::collections::BTreeMap;

use chrono::Utc;
use registry_api::{
    CATALOG_ARTIFACT, CATALOG_MEDIA_TYPE, CATALOG_REPOSITORY, Catalog, CatalogPackage,
    CatalogVersion, PackageAuth, Source, Target, Yanked,
    oci::{self, Manifest},
    signing::{self, Signatures},
};
use sqlx::types::Json;
use uuid::Uuid;

use super::{db, release};
use crate::{
    error::{ApiError, Result},
    state::AppState,
};

pub async fn changed(state: &AppState) {
    if state.harbor.is_none() {
        return;
    }
    if let Err(e) = sqlx::query!("UPDATE core_catalog SET dirty = true")
        .execute(&state.pool)
        .await
    {
        tracing::warn!("failed to mark the catalog dirty: {e}");
    }
    if let Err(e) = render(state).await {
        tracing::warn!("catalog render failed; the background job will retry: {e}");
    }
}

pub async fn render_if_dirty(state: &AppState) -> Result<Option<i64>> {
    if state.harbor.is_none() {
        return Ok(None);
    }
    let dirty = sqlx::query_scalar!("SELECT dirty FROM core_catalog WHERE id")
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or(false);
    if !dirty {
        return Ok(None);
    }
    render(state).await.map(Some)
}

async fn build(tx: &mut sqlx::PgConnection, packages_project: &str, seq: i64) -> Result<Catalog> {
    let packages = sqlx::query!(
        "SELECT id, name, description, binary_name, aliases, archived_at FROM cli_packages"
    )
    .fetch_all(&mut *tx)
    .await?;

    let versions = sqlx::query!(
        r#"SELECT package_id, version, targets, description, binary_name, aliases,
                  auth as "auth: Json<PackageAuth>", published_at as "published_at!",
                  published_by, source_repository, source_ref, source_sha, source_run_id,
                  yanked_at, yanked_by, yank_reason,
                  index_digest as "index_digest!", signatures_digest as "signatures_digest!"
           FROM cli_versions
           WHERE status = 'published' AND published_at IS NOT NULL
             AND index_digest IS NOT NULL AND signatures_digest IS NOT NULL"#
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut by_package: BTreeMap<Uuid, BTreeMap<String, CatalogVersion>> = BTreeMap::new();
    for v in versions {
        by_package.entry(v.package_id).or_default().insert(
            v.version,
            CatalogVersion {
                targets: v
                    .targets
                    .iter()
                    .filter_map(|t| t.parse::<Target>().ok())
                    .collect(),
                index: v.index_digest,
                signatures: v.signatures_digest,
                description: v.description,
                binary_name: v.binary_name,
                aliases: v.aliases,
                auth: v.auth.map(|a| a.0),
                published_at: v.published_at,
                published_by: v.published_by,
                source: v.source_repository.is_some().then_some(Source {
                    repository: v.source_repository,
                    git_ref: v.source_ref,
                    sha: v.source_sha,
                    run_id: v.source_run_id,
                }),
                yanked: v.yanked_at.map(|at| Yanked {
                    at,
                    by: v.yanked_by.unwrap_or_default(),
                    reason: v.yank_reason,
                }),
            },
        );
    }

    Ok(Catalog {
        seq,
        generated_at: Utc::now(),
        packages_project: packages_project.to_owned(),
        packages: packages
            .into_iter()
            .map(|p| {
                (
                    p.name,
                    CatalogPackage {
                        description: p.description,
                        binary_name: p.binary_name,
                        aliases: p.aliases,
                        archived: p.archived_at.is_some(),
                        versions: by_package.remove(&p.id).unwrap_or_default(),
                    },
                )
            })
            .collect(),
    })
}

pub async fn render(state: &AppState) -> Result<i64> {
    let (Some(harbor), Some(config)) = (&state.harbor, &state.config.harbor) else {
        return Ok(0);
    };

    let mut tx = state.pool.begin().await?;
    let seq = sqlx::query_scalar!("SELECT seq FROM core_catalog WHERE id FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?
        + 1;
    let catalog = build(&mut tx, &config.packages_project, seq).await?;

    let envelope = signing::sign(state.signer.key(), &catalog);
    let body = serde_json::to_vec(&envelope).expect("envelopes always serialise");
    let layer = harbor
        .oci()
        .push_bytes(CATALOG_REPOSITORY, CATALOG_MEDIA_TYPE, &body)
        .await?;
    harbor
        .oci()
        .push_bytes(CATALOG_REPOSITORY, oci::EMPTY, oci::EMPTY_JSON)
        .await?;
    let mut manifest = Manifest::artifact(CATALOG_ARTIFACT, vec![layer]);
    manifest.annotations.insert(
        "org.opencontainers.image.version".to_owned(),
        seq.to_string(),
    );
    let manifest = serde_json::to_vec(&manifest).expect("manifests always serialise");
    for tag in [seq.to_string(), "latest".to_owned()] {
        harbor
            .oci()
            .put_manifest(CATALOG_REPOSITORY, &tag, oci::MANIFEST, &manifest)
            .await?;
    }
    let digest = oci::digest(&manifest);
    super::cosign::sign(
        harbor,
        &state.signer,
        &config.registry,
        CATALOG_REPOSITORY,
        &digest,
    )
    .await?;

    sqlx::query!(
        "UPDATE core_catalog SET seq = $1, digest = $2, dirty = false, rendered_at = now()",
        seq,
        digest
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    tracing::info!(seq, digest, "rendered catalog");
    Ok(seq)
}

pub async fn backfill(state: &AppState) -> Result<usize> {
    let (Some(harbor), Some(config)) = (&state.harbor, &state.config.harbor) else {
        return Err(ApiError::Harbor("harbor is not configured".to_owned()));
    };

    let pending = sqlx::query!(
        r#"SELECT v.id, p.name, v.version
           FROM cli_versions v JOIN cli_packages p ON p.id = v.package_id
           WHERE v.status = 'published' AND v.signatures IS NOT NULL
             AND (v.index_digest IS NULL OR v.signatures_digest IS NULL)
           ORDER BY p.name, v.published_at"#
    )
    .fetch_all(&state.pool)
    .await?;

    let mut done = 0;
    for row in &pending {
        let package = db::find_package(&state.pool, &row.name).await?;
        let version = db::get_version(&state.pool, &package, &row.version).await?;
        let Some(Json(signatures)) = version.signatures.clone() else {
            continue;
        };
        let artifacts = db::artifacts(&state.pool, version.id).await?;

        match release::release(
            state,
            harbor,
            &config.packages_project,
            &row.name,
            &version,
            &artifacts,
            &signatures as &Signatures,
        )
        .await
        {
            Ok(released) => {
                sqlx::query!(
                    "UPDATE cli_versions SET index_digest = $2, signatures_digest = $3, cosign_signed = true
                     WHERE id = $1",
                    row.id,
                    released.index,
                    released.signatures
                )
                .execute(&state.pool)
                .await?;
                done += 1;
                tracing::info!(
                    package = row.name,
                    version = row.version,
                    "backfilled into harbor"
                );
            }
            Err(e) => {
                tracing::warn!(
                    package = row.name,
                    version = row.version,
                    "backfill failed: {e}"
                );
            }
        }
    }

    if done > 0 {
        changed(state).await;
    }
    Ok(done)
}

pub async fn current(
    axum::extract::State(state): axum::extract::State<AppState>,
    principal: crate::auth::Principal,
) -> Result<axum::Json<registry_api::CatalogRef>> {
    principal.require_reader()?;
    let row = sqlx::query!("SELECT seq, digest FROM core_catalog WHERE id")
        .fetch_one(&state.pool)
        .await?;
    Ok(axum::Json(registry_api::CatalogRef {
        seq: row.seq,
        digest: row.digest,
    }))
}

pub async fn sign_unsigned(state: &AppState) -> Result<usize> {
    let (Some(harbor), Some(config)) = (&state.harbor, &state.config.harbor) else {
        return Ok(0);
    };

    let unsigned = sqlx::query!(
        r#"SELECT v.id, p.name, v.version, v.index_digest as "index_digest!"
           FROM cli_versions v JOIN cli_packages p ON p.id = v.package_id
           WHERE v.status = 'published' AND v.index_digest IS NOT NULL AND NOT v.cosign_signed"#
    )
    .fetch_all(&state.pool)
    .await?;

    let mut signed = 0;
    for row in &unsigned {
        let repository = registry_api::package_repository(&config.packages_project, &row.name);
        match super::cosign::sign(
            harbor,
            &state.signer,
            &config.registry,
            &repository,
            &row.index_digest,
        )
        .await
        {
            Ok(()) => {
                sqlx::query!(
                    "UPDATE cli_versions SET cosign_signed = true WHERE id = $1",
                    row.id
                )
                .execute(&state.pool)
                .await?;
                signed += 1;
            }
            Err(e) => {
                tracing::warn!(
                    package = row.name,
                    version = row.version,
                    "cosign signing failed: {e}"
                );
            }
        }
    }

    let current = sqlx::query_scalar!("SELECT digest FROM core_catalog WHERE id")
        .fetch_optional(&state.pool)
        .await?
        .flatten();
    if let Some(digest) = current
        && harbor
            .oci()
            .manifest(CATALOG_REPOSITORY, &super::cosign::signature_tag(&digest))
            .await?
            .is_none()
    {
        super::cosign::sign(
            harbor,
            &state.signer,
            &config.registry,
            CATALOG_REPOSITORY,
            &digest,
        )
        .await?;
        signed += 1;
    }
    Ok(signed)
}
