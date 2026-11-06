mod chat;
mod mutation;
mod objects;
mod query;
mod subscription;

use async_graphql::{Context, Error, ErrorExtensions, Schema, SchemaBuilder};
use subgraph::node::Ids;
use subgraph::{AuthConfig, Authenticator, Claims, Provider};

use crate::error::GatewayError;
use crate::keys::VirtualKey;
use crate::state::AppState;

pub use mutation::MutationRoot;
pub use query::QueryRoot;
pub use subscription::SubscriptionRoot;

pub const IDS: Ids = Ids::new("aiGateway");

pub fn builder() -> SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .register_output_type::<objects::Node>()
        .enable_federation()
        .enable_subscription_in_federation()
}

pub fn router(state: AppState) -> Option<axum::Router> {
    let graph = state.config.graph.as_ref()?;
    let groups = state
        .config
        .oauth
        .as_ref()
        .map(|oauth| oauth.groups.clone())
        .filter(|groups| !groups.is_empty())?;
    let auth = Authenticator::new(AuthConfig {
        providers: vec![Provider {
            issuer: graph.issuer.clone(),
            audience: graph.audience.clone(),
        }],
        groups,
    });
    tokio::spawn(auth.clone().run());

    let schema = builder().data(state).finish();
    Some(subgraph::router(schema, auth))
}

fn state<'a>(ctx: &Context<'a>) -> &'a AppState {
    ctx.data_unchecked::<AppState>()
}

async fn user_key(ctx: &Context<'_>) -> async_graphql::Result<VirtualKey> {
    let claims = ctx.data::<Claims>()?;
    state(ctx)
        .keys
        .for_user(&claims.sub, claims.identity())
        .await
        .gql()
}

trait Gql<T> {
    fn gql(self) -> async_graphql::Result<T>;
}

impl<T> Gql<T> for Result<T, GatewayError> {
    fn gql(self) -> async_graphql::Result<T> {
        self.map_err(gateway_error)
    }
}

fn gateway_error(e: GatewayError) -> Error {
    let code = match &e {
        GatewayError::MissingKey | GatewayError::InvalidKey => "UNAUTHENTICATED",
        GatewayError::ModelNotAllowed(..) | GatewayError::ModelDenied(_) => "FORBIDDEN",
        GatewayError::BudgetExceeded(_) => "BUDGET_EXCEEDED",
        GatewayError::NoProvider(_) | GatewayError::BadRequest(_) => "BAD_USER_INPUT",
        GatewayError::Disabled => "UNAVAILABLE",
        GatewayError::Upstream(_) => "UPSTREAM",
        GatewayError::Database(_) => "INTERNAL_SERVER_ERROR",
    };
    let message = match &e {
        GatewayError::Database(inner) => {
            tracing::error!("graphql request failed: {inner}");
            "internal error".to_owned()
        }
        other => other.to_string(),
    };
    Error::new(message).extend_with(|_, ext| ext.set("code", code))
}

fn not_found(what: impl std::fmt::Display) -> Error {
    Error::new(format!("{what} not found")).extend_with(|_, ext| ext.set("code", "NOT_FOUND"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn subgraph_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("subgraph.graphql");
        subgraph::assert_subgraph_current(&super::builder().finish(), path);
    }
}
