pub mod artifacts;
pub mod db;
pub mod packages;
pub mod versions;

use axum::{
    Router,
    routing::{get, post},
};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/packages", get(packages::list).post(packages::create))
        .route(
            "/packages/{name}",
            get(packages::get)
                .patch(packages::update)
                .delete(packages::delete),
        )
        .route("/packages/{name}/latest", get(packages::latest))
        .route("/packages/{name}/versions", post(versions::create))
        .route("/packages/{name}/versions/{version}", get(versions::get))
        .route(
            "/packages/{name}/versions/{version}/publish",
            post(versions::publish),
        )
        .route(
            "/packages/{name}/versions/{version}/yank",
            post(versions::yank),
        )
        .route(
            "/packages/{name}/versions/{version}/unyank",
            post(versions::unyank),
        )
        .route(
            "/packages/{name}/versions/{version}/artifacts/{target}",
            get(artifacts::download).put(artifacts::upload),
        )
        .route(
            "/packages/{name}/versions/{version}/artifacts/{target}/url",
            get(artifacts::download_url),
        )
}
