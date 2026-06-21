mod types;

pub use types::{KeyInfo, UpdateKey, VirtualKey};

use rand::RngExt;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::cache::CacheClient;
use crate::error::{GatewayError, Result};
use types::KeyRow;

const KEY_PREFIX: &str = "aig_";
const KEY_BYTES: usize = 24;
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

const KEY_CACHE_TTL: u64 = 3600;
const BUDGET_CACHE_TTL: u64 = 60;

/// Cache-key namespaces in the shared dragonfly store. `KEY_NAMESPACE` is flushed wholesale
/// on any key mutation, so it gets its own prefix the SCAN pattern can target.
const KEY_NAMESPACE: &str = "aig:key:";

fn key_cache_key(hash: &str) -> String {
    format!("{KEY_NAMESPACE}{hash}")
}

fn budget_key(id: Uuid) -> String {
    format!("aig:budget:{id}")
}

#[derive(Clone)]
pub struct KeyStore {
    pool: PgPool,
    /// Shared across replicas so cached keys, throttles and budgets stay consistent. When
    /// absent (no `REDIS_URL`) every lookup falls back to Postgres.
    cache: Option<CacheClient>,
}

impl KeyStore {
    pub fn new(pool: PgPool, cache: Option<CacheClient>) -> Self {
        Self { pool, cache }
    }

    /// Flushes every cached virtual key across the fleet so a mutation takes effect
    /// immediately rather than lingering for the TTL.
    async fn invalidate_keys(&self) {
        if let Some(cache) = &self.cache {
            cache.invalidate(&format!("{KEY_NAMESPACE}*")).await;
        }
    }

    fn hash(raw: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Authenticates a raw bearer token. Revoked keys are treated as absent.
    #[tracing::instrument(skip_all, fields(otel.name = "key.authenticate"))]
    pub async fn authenticate(&self, raw: &str) -> Result<VirtualKey> {
        let hash = Self::hash(raw);

        if let Some(cache) = &self.cache
            && let Some(key) = cache.get_json::<VirtualKey>(&key_cache_key(&hash)).await
        {
            return Ok(key);
        }

        struct QueryRow {
            id: Uuid,
            name: String,
            allowed_models: Vec<String>,
            monthly_token_budget: Option<i64>,
        }

        let key = sqlx::query_as!(
            QueryRow,
            "SELECT id, name, allowed_models, monthly_token_budget FROM virtual_keys \
             WHERE key_hash = $1 AND revoked = FALSE",
            &hash
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|row| VirtualKey {
            id: row.id,
            name: row.name,
            allowed_models: row.allowed_models,
            monthly_token_budget: row.monthly_token_budget,
        })
        .ok_or(GatewayError::InvalidKey)?;

        if let Some(cache) = &self.cache {
            cache
                .set_json(&key_cache_key(&hash), KEY_CACHE_TTL, &key)
                .await;
        }
        Ok(key)
    }

    pub async fn month_to_date_tokens(&self, id: Uuid) -> Result<i64> {
        if let Some(cache) = &self.cache
            && let Some(total) = cache.get_i64(&budget_key(id)).await
        {
            return Ok(total);
        }

        let total = sqlx::query_scalar!(
            "SELECT COALESCE(SUM(input_tokens + output_tokens), 0)::bigint \
             FROM usage_events \
             WHERE key_id = $1 AND created_at >= date_trunc('month', now())",
            id
        )
        .fetch_one(&self.pool)
        .await?
        .unwrap_or(0);

        if let Some(cache) = &self.cache {
            cache.set_i64(&budget_key(id), BUDGET_CACHE_TTL, total).await;
        }
        Ok(total)
    }

    /// Creates a key and returns the one-time plaintext token alongside its row.
    pub async fn create(
        &self,
        name: &str,
        allowed_models: &[String],
        monthly_token_budget: Option<i64>,
    ) -> Result<(String, KeyInfo)> {
        let raw = generate_token();
        let hash = Self::hash(&raw);

        let info = sqlx::query_as!(
            KeyRow,
            "INSERT INTO virtual_keys (name, key_hash, allowed_models, monthly_token_budget) \
             VALUES ($1, $2, $3, $4) \
             RETURNING id, name, allowed_models, monthly_token_budget, revoked, created_at",
            name,
            hash,
            allowed_models,
            monthly_token_budget
        )
        .fetch_one(&self.pool)
        .await?
        .into();

        Ok((raw, info))
    }

    /// Applies config-managed fields to an existing key, matched by name. The key itself
    /// is minted via the admin API; config is the source of truth for every mutable field
    /// (everything but id and created_at). Returns `false` if no key with
    /// that name exists yet.
    pub async fn claim(
        &self,
        name: &str,
        allowed_models: &[String],
        monthly_token_budget: Option<i64>,
        revoked: bool,
    ) -> Result<bool> {
        // Only writes (and thus only invalidates the cache) when a field actually differs,
        // so a fleet rollout re-claiming unchanged keys doesn't stampede the cache. `found`
        // still reflects existence so the caller can warn about keys missing from the DB.
        let row = sqlx::query!(
            r#"
            WITH existing AS (
                SELECT id,
                       (allowed_models IS DISTINCT FROM $2
                         OR monthly_token_budget IS DISTINCT FROM $3
                         OR revoked IS DISTINCT FROM $4) AS changed
                FROM virtual_keys WHERE name = $1
            ),
            updated AS (
                UPDATE virtual_keys SET
                    allowed_models = $2,
                    monthly_token_budget = $3,
                    revoked = $4
                FROM existing
                WHERE virtual_keys.id = existing.id AND existing.changed
                RETURNING virtual_keys.id
            )
            SELECT
                EXISTS (SELECT 1 FROM existing) AS "found!",
                EXISTS (SELECT 1 FROM updated) AS "changed!"
            "#,
            name,
            allowed_models,
            monthly_token_budget,
            revoked
        )
        .fetch_one(&self.pool)
        .await?;

        if row.changed {
            self.invalidate_keys().await;
        }
        Ok(row.found)
    }

    /// Mints a fresh token for an existing key, replacing its hash, and returns the one-time
    /// plaintext alongside the row. The old token stops authenticating immediately once the
    /// cache is flushed. Returns `None` if no key has that id.
    pub async fn regenerate(&self, id: Uuid) -> Result<Option<(String, KeyInfo)>> {
        let raw = generate_token();
        let hash = Self::hash(&raw);

        let info = sqlx::query_as!(
            KeyRow,
            "UPDATE virtual_keys SET key_hash = $2 WHERE id = $1 \
             RETURNING id, name, allowed_models, monthly_token_budget, revoked, created_at",
            id,
            hash,
        )
        .fetch_optional(&self.pool)
        .await?
        .map(KeyInfo::from);

        match info {
            Some(info) => {
                self.invalidate_keys().await;
                Ok(Some((raw, info)))
            }
            None => Ok(None),
        }
    }

    pub async fn list(&self) -> Result<Vec<KeyInfo>> {
        let rows = sqlx::query_as!(
            KeyRow,
            "SELECT id, name, allowed_models, monthly_token_budget, revoked, created_at \
             FROM virtual_keys ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Revokes (soft-deletes) a key, flushing the cache so it stops authenticating
    /// immediately rather than lingering for the cache TTL.
    pub async fn revoke(&self, id: Uuid) -> Result<bool> {
        let affected = sqlx::query!("UPDATE virtual_keys SET revoked = TRUE WHERE id = $1", id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected > 0 {
            self.invalidate_keys().await;
        }
        Ok(affected > 0)
    }

    /// Applies a partial update; only the fields present in `fields` change. Returns the
    /// updated row, or `None` if no key has that id. Flushes the cache so changes take
    /// effect immediately rather than waiting out the TTL.
    pub async fn update(&self, id: Uuid, fields: &UpdateKey) -> Result<Option<KeyInfo>> {
        // keep as runtime because of COALESCE type inference
        let info = sqlx::query_as!(
           KeyRow,
            r#"
            UPDATE virtual_keys SET
                name = COALESCE($2::text, name),
                allowed_models = COALESCE($3::text[], allowed_models),
                monthly_token_budget = COALESCE($4::bigint, monthly_token_budget),
                revoked = COALESCE($5::boolean, revoked)
            WHERE id = $1::uuid
            RETURNING id, name, allowed_models, monthly_token_budget, revoked, created_at
            "#,
            id,
            fields.name,
            fields.allowed_models.as_deref(),
            fields.monthly_token_budget,
            fields.revoked,
        )
        .fetch_optional(&self.pool)
        .await?
        .map(Into::into);

        if info.is_some() {
            self.invalidate_keys().await;
        }
        Ok(info)
    }
}

fn generate_token() -> String {
    let mut rng = rand::rng();
    let body: String = (0..KEY_BYTES)
        .map(|_| BASE62[rng.random_range(0..BASE62.len())] as char)
        .collect();
    format!("{KEY_PREFIX}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_prefixed_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert!(a.starts_with(KEY_PREFIX));
        assert_eq!(a.len(), KEY_PREFIX.len() + KEY_BYTES);
        assert_ne!(a, b);
    }
}
