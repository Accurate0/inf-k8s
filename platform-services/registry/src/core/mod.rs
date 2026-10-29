pub mod audit;
pub mod publishers;
pub mod tokens;

use axum::{
    Json, Router,
    routing::{delete, get},
};
use registry_api::{ECOSYSTEM_CLI, WhoAmI};
use sqlx::PgPool;

use crate::{
    auth::Principal,
    error::{ApiError, Result},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/whoami", get(whoami))
        .route("/tokens", get(tokens::list).post(tokens::create))
        .route("/tokens/{id}", delete(tokens::revoke))
        .route(
            "/trusted-publishers",
            get(publishers::list).post(publishers::create),
        )
        .route("/trusted-publishers/{id}", delete(publishers::delete))
        .route("/audit", get(audit::list))
}

async fn whoami(principal: Principal) -> Json<WhoAmI> {
    let (groups, scopes) = match &principal {
        Principal::User { groups, .. } => (groups.clone(), Vec::new()),
        Principal::Token(token) => (Vec::new(), token.scopes.clone()),
        Principal::Forgejo(_) => (Vec::new(), Vec::new()),
    };

    Json(WhoAmI {
        subject: principal.actor(),
        kind: principal.kind().to_owned(),
        admin: principal.is_admin(),
        groups,
        scopes,
    })
}

pub async fn ensure_package_exists(pool: &PgPool, ecosystem: &str, package: &str) -> Result<()> {
    let exists: bool = match ecosystem {
        ECOSYSTEM_CLI => {
            sqlx::query_scalar!(
                r#"SELECT EXISTS (SELECT 1 FROM cli_packages WHERE name = $1) AS "exists!""#,
                package
            )
            .fetch_one(pool)
            .await?
        }
        other => return Err(ApiError::BadRequest(format!("unknown ecosystem {other:?}"))),
    };

    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!(
            "{ecosystem} package {package} does not exist"
        )))
    }
}
