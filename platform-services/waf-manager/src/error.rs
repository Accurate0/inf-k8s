#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("kube error: {0}")]
    Kube(#[from] kube::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("invalid cidr {0:?}: {1}")]
    InvalidCidr(String, String),

    #[error("refusing to block {0}: overlaps protected range {1}")]
    ProtectedRange(String, String),

    #[error("refusing to allowlist {0}: block {1} covers it, unblock it first")]
    AllowlistConflict(String, String),

    #[error("unexpected response from loki: {0}")]
    Loki(String),

    #[error("template render failed: {0}")]
    Render(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("jwks error: {0}")]
    Jwks(String),

    #[error("sync failed: {0}")]
    Sync(String),

    #[error("object {0} is missing a namespace")]
    MissingNamespace(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
