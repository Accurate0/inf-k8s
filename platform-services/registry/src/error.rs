use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use registry_api::{ErrorBody, ErrorDetail};

#[derive(thiserror::Error, Debug)]
pub enum ApiError {
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{1}")]
    Conflict(&'static str, String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unprocessable(String),
    #[error("artifact exceeds the {0} byte limit")]
    TooLarge(u64),
    #[error("authentication unavailable: {0}")]
    AuthUnavailable(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("database error: {0}")]
    Database(Box<sqlx::Error>),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Database(Box::new(e))
    }
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(..) => StatusCode::CONFLICT,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::AuthUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Storage(_) => StatusCode::BAD_GATEWAY,
            ApiError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized(_) => "unauthorized",
            ApiError::Forbidden(_) => "forbidden",
            ApiError::NotFound(_) => "not_found",
            ApiError::Conflict(code, _) => code,
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Unprocessable(_) => "unprocessable",
            ApiError::TooLarge(_) => "too_large",
            ApiError::AuthUnavailable(_) => "auth_unavailable",
            ApiError::Storage(_) => "storage",
            ApiError::Database(_) => "internal",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!("request failed: {self}");
        } else {
            tracing::warn!("request rejected: {self}");
        }

        let message = match &self {
            ApiError::Database(_) => "internal error".to_owned(),
            other => other.to_string(),
        };

        let body = Json(ErrorBody {
            error: ErrorDetail {
                code: self.code().to_owned(),
                message,
            },
        });

        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;
