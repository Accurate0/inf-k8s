use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use lambda_http::tracing;

pub enum AppError {
    Error(anyhow::Error),
    #[allow(dead_code)]
    StatusCode(StatusCode),
    #[allow(dead_code)]
    Message(StatusCode, String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Error(e) => {
                tracing::error!("an app error occurred: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Something went wrong: {}", e),
                )
                    .into_response()
            }
            AppError::StatusCode(s) => {
                (s, s.canonical_reason().unwrap_or("").to_owned()).into_response()
            }
            AppError::Message(s, msg) => (s, msg).into_response(),
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self::Error(err.into())
    }
}
