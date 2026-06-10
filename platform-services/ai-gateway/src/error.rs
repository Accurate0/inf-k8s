use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(thiserror::Error, Debug)]
pub enum GatewayError {
    #[error("missing or malformed Authorization header")]
    MissingKey,
    #[error("invalid or revoked api key")]
    InvalidKey,
    #[error("key {0} is not allowed to use model {1}")]
    ModelNotAllowed(String, String),
    #[error("key {0} has exceeded its monthly token budget")]
    BudgetExceeded(String),
    #[error("no provider configured for model {0}")]
    NoProvider(String),
    #[error("gateway disabled by feature flag")]
    Disabled,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream request failed: {0}")]
    Upstream(Box<reqwest::Error>),
    #[error("database error: {0}")]
    Database(Box<sqlx::Error>),
}

impl From<reqwest::Error> for GatewayError {
    fn from(e: reqwest::Error) -> Self {
        GatewayError::Upstream(Box::new(e))
    }
}

impl From<sqlx::Error> for GatewayError {
    fn from(e: sqlx::Error) -> Self {
        GatewayError::Database(Box::new(e))
    }
}

impl GatewayError {
    fn status(&self) -> StatusCode {
        match self {
            GatewayError::MissingKey | GatewayError::InvalidKey => StatusCode::UNAUTHORIZED,
            GatewayError::ModelNotAllowed(..) => StatusCode::FORBIDDEN,
            GatewayError::BudgetExceeded(_) => StatusCode::TOO_MANY_REQUESTS,
            GatewayError::NoProvider(_) | GatewayError::BadRequest(_) => StatusCode::BAD_REQUEST,
            GatewayError::Disabled => StatusCode::SERVICE_UNAVAILABLE,
            GatewayError::Upstream(_) => StatusCode::BAD_GATEWAY,
            GatewayError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status == StatusCode::INTERNAL_SERVER_ERROR || status == StatusCode::BAD_GATEWAY {
            tracing::error!("request failed: {self}");
        } else {
            tracing::warn!("request rejected: {self}");
        }

        let body = Json(json!({
            "error": {
                "type": "ai_gateway_error",
                "message": self.to_string(),
            }
        }));

        let mut response = (status, body).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert("Retry-After", "30".parse().unwrap());
        }
        response
    }
}

pub type Result<T> = std::result::Result<T, GatewayError>;
