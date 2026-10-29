use rand::RngExt;
use registry_api::{Scope, TOKEN_PREFIX};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const TOKEN_CHARS: usize = 43;
const DISPLAY_PREFIX_LEN: usize = 12;

pub fn generate() -> String {
    let mut rng = rand::rng();
    let body: String = (0..TOKEN_CHARS)
        .map(|_| BASE62[rng.random_range(0..BASE62.len())] as char)
        .collect();
    format!("{TOKEN_PREFIX}{body}")
}

pub fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn display_prefix(token: &str) -> String {
    token.chars().take(DISPLAY_PREFIX_LEN).collect()
}

#[derive(Debug, Clone)]
pub struct TokenPrincipal {
    pub id: Uuid,
    pub name: String,
    pub scopes: Vec<Scope>,
}

impl TokenPrincipal {
    pub fn allows(&self, ecosystem: &str, package: &str) -> bool {
        self.scopes
            .iter()
            .any(|s| s.ecosystem == ecosystem && s.package == package)
    }
}

pub async fn lookup(pool: &PgPool, token: &str) -> sqlx::Result<Option<TokenPrincipal>> {
    let row = sqlx::query!(
        "UPDATE core_publish_tokens SET last_used_at = now()
         WHERE token_hash = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())
         RETURNING id, name",
        hash(token)
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let (id, name) = (row.id, row.name);

    let scopes = sqlx::query_as!(
        Scope,
        "SELECT ecosystem, package FROM core_publish_token_scopes WHERE token_id = $1",
        id
    )
    .fetch_all(pool)
    .await?;

    Ok(Some(TokenPrincipal { id, name, scopes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_prefixed_and_unique() {
        let a = generate();
        let b = generate();

        assert!(a.starts_with(TOKEN_PREFIX));
        assert_eq!(a.len(), TOKEN_PREFIX.len() + TOKEN_CHARS);
        assert_ne!(a, b);
        assert_ne!(hash(&a), hash(&b));
        assert_eq!(display_prefix(&a).len(), DISPLAY_PREFIX_LEN);
    }
}
