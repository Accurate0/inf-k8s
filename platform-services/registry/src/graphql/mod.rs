mod mutation;
mod objects;
mod query;
mod subscription;

use async_graphql::{Context, Error, ErrorExtensions, Schema, SchemaBuilder};
use subgraph::node::Ids;
use subgraph::{AuthConfig, Authenticator, Claims, Provider};

use crate::auth::Principal;
use crate::auth::oidc::groups_match;
use crate::error::ApiError;
use crate::state::AppState;

pub use mutation::MutationRoot;
pub use query::QueryRoot;
pub use subscription::SubscriptionRoot;

pub type RegistrySchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub const IDS: Ids = Ids::new("registry");

pub fn builder() -> SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .register_output_type::<objects::Node>()
        .enable_federation()
        .enable_subscription_in_federation()
        .extension(subgraph::Tracing)
}

pub fn router(state: AppState) -> Option<axum::Router> {
    let graph = state.config.graph.as_ref()?;
    let oauth = &state.config.oauth;
    let auth = Authenticator::new(AuthConfig {
        providers: vec![Provider {
            issuer: graph.issuer.clone(),
            audience: graph.audience.clone(),
        }],
        groups: oauth
            .read_groups
            .iter()
            .chain(&oauth.admin_groups)
            .cloned()
            .collect(),
    });
    tokio::spawn(auth.clone().run());

    let schema = builder().data(state).finish();
    Some(subgraph::router(schema, auth))
}

fn state<'a>(ctx: &Context<'a>) -> &'a AppState {
    ctx.data_unchecked::<AppState>()
}

fn principal(ctx: &Context<'_>) -> async_graphql::Result<Principal> {
    let claims = ctx.data::<Claims>()?;
    let groups = claims.groups.clone().unwrap_or_default();
    let oauth = &state(ctx).config.oauth;

    Ok(Principal::User {
        name: claims.identity().to_owned(),
        admin: groups_match(&groups, &oauth.admin_groups),
        reader: groups_match(&groups, &oauth.read_groups),
        groups,
    })
}

// Service account tokens only act for the forgejo run they were exchanged for.
// Kanidm omits preferred_username without the profile scope, so exchanges are found by jti
// rather than by the token's identity.
async fn deployer(ctx: &Context<'_>) -> async_graphql::Result<Principal> {
    let principal = principal(ctx)?;
    let claims = ctx.data::<Claims>()?;
    let state = state(ctx);

    let exchanged = match &claims.jti {
        Some(jti) => crate::core::graph_exchange::exchanged_for(state, jti)
            .await
            .gql()?,
        None => None,
    };
    if let Some((account, forgejo)) = exchanged {
        return Ok(Principal::Exchanged {
            account,
            groups: principal.groups().to_vec(),
            forgejo,
        });
    }

    let exchange_account = state
        .config
        .graph
        .as_ref()
        .and_then(|g| g.exchange.as_ref())
        .and_then(|e| e.accounts.iter().find(|a| a.name == claims.identity()));
    if let Some(account) = exchange_account {
        return Err(Error::new(format!(
            "{} tokens must come from the registry's graph token exchange",
            account.name
        ))
        .extend_with(|_, ext| ext.set("code", "forbidden")));
    }

    Ok(principal)
}

trait Gql<T> {
    fn gql(self) -> async_graphql::Result<T>;
}

impl<T> Gql<T> for Result<T, ApiError> {
    fn gql(self) -> async_graphql::Result<T> {
        self.map_err(|e| {
            let message = match &e {
                ApiError::Database(inner) => {
                    tracing::error!("graphql request failed: {inner}");
                    "internal error".to_owned()
                }
                other => other.to_string(),
            };
            let code = e.code();
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
