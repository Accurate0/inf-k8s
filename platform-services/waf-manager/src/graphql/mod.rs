mod objects;
mod roots;

use std::sync::Arc;

use async_graphql::{Context, Error, ErrorExtensions, Schema, SchemaBuilder};
use subgraph::node::Ids;
use subgraph::{AuthConfig, Authenticator};

use crate::routes::{AppError, AppState};

pub use roots::{MutationRoot, QueryRoot, SubscriptionRoot};

pub const IDS: Ids = Ids::new("waf");

pub fn builder() -> SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .register_output_type::<objects::Node>()
        .enable_federation()
        .enable_subscription_in_federation()
        .extension(subgraph::Tracing)
}

pub fn router(state: Arc<AppState>, auth: AuthConfig) -> axum::Router {
    let auth = Authenticator::new(auth);
    tokio::spawn(auth.clone().run());

    let schema = builder().data(state).finish();
    subgraph::router(schema, auth)
}

fn state<'a>(ctx: &Context<'a>) -> &'a Arc<AppState> {
    ctx.data_unchecked::<Arc<AppState>>()
}

/// The audit actor for a graph user, matching what the UI and CLI record.
fn claims(ctx: &Context<'_>) -> async_graphql::Result<waf_manager::Claims> {
    let claims = ctx.data::<subgraph::Claims>()?;
    Ok(waf_manager::Claims {
        sub: claims.sub.clone(),
        preferred_username: claims.preferred_username.clone(),
        email: None,
        name: None,
    })
}

trait Gql<T> {
    fn gql(self) -> async_graphql::Result<T>;
}

impl<T> Gql<T> for Result<T, AppError> {
    fn gql(self) -> async_graphql::Result<T> {
        self.map_err(|AppError(e)| {
            use waf_manager::Error as WafError;
            let code = match &e {
                WafError::ProtectedRange(..) | WafError::InvalidCidr(..) => "BAD_USER_INPUT",
                WafError::AllowlistConflict(..) => "CONFLICT",
                WafError::Unauthorized(_) => "UNAUTHENTICATED",
                WafError::Kube(kube::Error::Api(status)) if status.code == 404 => "NOT_FOUND",
                WafError::Kube(kube::Error::Api(status)) if status.code == 409 => "CONFLICT",
                _ => "INTERNAL_SERVER_ERROR",
            };
            let message = if code == "INTERNAL_SERVER_ERROR" {
                tracing::warn!("graphql request failed: {e}");
                "internal error".to_owned()
            } else {
                e.to_string()
            };
            Error::new(message).extend_with(|_, ext| ext.set("code", code))
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn subgraph_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("subgraph.graphql");
        subgraph::assert_subgraph_current(&super::builder().finish(), path);
    }
}
