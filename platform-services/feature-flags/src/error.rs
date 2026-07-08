use tonic::Status;

/// Errors from the store/snapshot layers. gRPC handlers map these to `Status`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("aborted: {0}")]
    Aborted(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<AppError> for Status {
    fn from(e: AppError) -> Self {
        match e {
            AppError::NotFound(m) => Status::not_found(m),
            AppError::Invalid(m) => Status::invalid_argument(m),
            AppError::Aborted(m) => Status::aborted(m),
            AppError::Sqlx(e) => Status::internal(format!("database error: {e}")),
            AppError::Other(e) => Status::internal(e.to_string()),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
