#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("kube error: {0}")]
    Kube(#[from] kube::Error),

    #[error("invalid postgres identifier: {0:?}")]
    InvalidIdentifier(String),

    #[error("invalid spec: {0}")]
    Validation(String),

    #[error("object {0} is missing a namespace")]
    MissingNamespace(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
