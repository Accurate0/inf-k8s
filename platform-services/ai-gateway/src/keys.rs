use std::time::Duration;

use chrono::{DateTime, Utc};
use moka::future::Cache;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{GatewayError, Result};

const KEY_PREFIX: &str = "aig_";
const KEY_BYTES: usize = 24;
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A resolved virtual key, as seen on the request hot path.
#[derive(Clone, Debug)]
pub struct VirtualKey {
    pub id: Uuid,
    pub name: String,
    pub allowed_models: Vec<String>,
}

impl VirtualKey {
    /// Empty `allowed_models` means "any model".
    pub fn allows(&self, model: &str) -> bool {
        self.allowed_models.is_empty() || self.allowed_models.iter().any(|m| m == model)
    }
}

#[derive(Clone, Serialize)]
pub struct KeyInfo {
    pub id: Uuid,
    pub name: String,
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i64>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct KeyStore {
    pool: PgPool,
    /// Maps key hash -> resolved key, so the hot path avoids Postgres per request.
    cache: Cache<String, VirtualKey>,
}

impl KeyStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            cache: Cache::builder()
                .name("virtual_key_cache")
                .time_to_live(Duration::from_secs(3600))
                .build(),
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

        if let Some(key) = self.cache.get(&hash).await {
            self.touch(key.id).await;
            return Ok(key);
        }

        struct QueryRow {
            id: Uuid,
            name: String,
            allowed_models: Vec<String>,
        }

        let key = sqlx::query_as!(
            QueryRow,
            "SELECT id, name, allowed_models FROM virtual_keys \
             WHERE key_hash = $1 AND revoked = FALSE",
            &hash
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|row| VirtualKey {
            id: row.id,
            name: row.name,
            allowed_models: row.allowed_models,
        })
        .ok_or(GatewayError::InvalidKey)?;

        self.cache.insert(hash, key.clone()).await;
        self.touch(key.id).await;
        Ok(key)
    }

    /// Best-effort `last_used_at` bump; never fails the request.
    async fn touch(&self, id: Uuid) {
        if let Err(e) = sqlx::query!(
            "UPDATE virtual_keys SET last_used_at = now() WHERE id = $1",
            id
        )
        .execute(&self.pool)
        .await
        {
            tracing::debug!("failed to bump last_used_at for {id}: {e}");
        }
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
             RETURNING id, name, allowed_models, monthly_token_budget, revoked, created_at, last_used_at",
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

    pub async fn list(&self) -> Result<Vec<KeyInfo>> {
        let rows = sqlx::query_as!(
            KeyRow,
            "SELECT id, name, allowed_models, monthly_token_budget, revoked, created_at, last_used_at \
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
            self.cache.invalidate_all();
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
            RETURNING id, name, allowed_models, monthly_token_budget, revoked, created_at, last_used_at
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
            self.cache.invalidate_all();
        }
        Ok(info)
    }
}

/// Partial update payload; absent fields are left unchanged. `monthly_token_budget`
/// can only be set, not cleared back to null, through this path.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateKey {
    pub name: Option<String>,
    pub allowed_models: Option<Vec<String>>,
    pub monthly_token_budget: Option<i64>,
    pub revoked: Option<bool>,
}

#[derive(sqlx::FromRow)]
struct KeyRow {
    id: Uuid,
    name: String,
    allowed_models: Vec<String>,
    monthly_token_budget: Option<i64>,
    revoked: bool,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

impl From<KeyRow> for KeyInfo {
    fn from(r: KeyRow) -> Self {
        KeyInfo {
            id: r.id,
            name: r.name,
            allowed_models: r.allowed_models,
            monthly_token_budget: r.monthly_token_budget,
            revoked: r.revoked,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        }
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

    #[test]
    fn empty_allowlist_permits_any_model() {
        let key = VirtualKey {
            id: Uuid::nil(),
            name: "t".into(),
            allowed_models: vec![],
        };
        assert!(key.allows("anything"));
    }

    #[test]
    fn allowlist_restricts_models() {
        let key = VirtualKey {
            id: Uuid::nil(),
            name: "t".into(),
            allowed_models: vec!["claude-fable-5".into()],
        };
        assert!(key.allows("claude-fable-5"));
        assert!(!key.allows("gpt-4o"));
    }
}
