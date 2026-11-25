mod mutation;
mod objects;
mod publishing;
mod query;
mod subscription;

use async_graphql::{Context, Error, ErrorExtensions, Schema, SchemaBuilder};
use subgraph::external::OneOrMany;
use subgraph::node::Ids;
use subgraph::{
    AuthConfig, Authenticator, ClaimPattern, Claims, External, ExternalAuth, ExternalIssuer,
    Provider,
};

use crate::auth::scopes;
use crate::auth::{ForgejoClaims, Principal};
use crate::error::ApiError;
use crate::state::AppState;

pub use mutation::MutationRoot;
pub use query::QueryRoot;
pub use subscription::SubscriptionRoot;

pub type RegistrySchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

pub const IDS: Ids = Ids::new("registry");

const FORGEJO: &str = "forgejo";

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
    let groups = oauth
        .read_groups
        .iter()
        .chain(&oauth.admin_groups)
        .cloned()
        .collect();

    let config = match state.config.subgraph_auth.clone() {
        // groups keep admitting callers whose tokens predate the registry's scopes
        Some(auth) => AuthConfig { groups, ..auth },
        None => AuthConfig {
            providers: vec![Provider {
                issuer: graph.issuer.clone(),
                audience: graph.audience.clone(),
            }],
            groups,
            scopes: Vec::new(),
            external: forgejo(&state, graph).into_iter().collect(),
        },
    };
    let auth = Authenticator::new(config);
    tokio::spawn(auth.clone().run());

    let schema = builder().data(state).finish();
    Some(subgraph::router(schema, auth))
}

/// CI reaches the registry through the graph with its forgejo actions token; trusted
/// publishers and grants decide what each repository may do.
fn forgejo(state: &AppState, graph: &crate::config::GraphConfig) -> Option<ExternalAuth> {
    let forgejo = state.config.forgejo.as_ref()?;
    let audience = graph.forgejo_audience.clone()?;
    let base = forgejo.issuer.trim_end_matches('/');

    Some(ExternalAuth {
        issuer: ExternalIssuer {
            name: FORGEJO.to_owned(),
            issuer: forgejo.issuer.clone(),
            jwks: forgejo
                .jwks_uri
                .clone()
                .unwrap_or_else(|| format!("{base}/.well-known/keys")),
            audience,
        },
        patterns: vec![ClaimPattern {
            issuer: FORGEJO.to_owned(),
            claims: [("repository".to_owned(), OneOrMany::One("*".to_owned()))].into(),
        }],
        grants: Vec::new(),
    })
}

fn state<'a>(ctx: &Context<'a>) -> &'a AppState {
    ctx.data_unchecked::<AppState>()
}

fn principal(ctx: &Context<'_>) -> async_graphql::Result<Principal> {
    if let Some(external) = ctx.data_opt::<External>() {
        if external.issuer != FORGEJO {
            return Err(Error::new(format!(
                "{} tokens are not accepted",
                external.issuer
            )));
        }
        let mut claims = external.claims_as::<ForgejoClaims>()?;
        claims.scopes = if state(ctx).config.subgraph_auth.is_some() {
            external.scopes.clone()
        } else {
            scopes::owned(&scopes::TOKEN)
        };
        return Ok(Principal::Forgejo(claims));
    }

    let claims = ctx.data::<Claims>()?;
    let oauth = &state(ctx).config.oauth;
    Ok(crate::auth::user(
        &oauth.admin_groups,
        &oauth.read_groups,
        claims.identity(),
        claims.scope.as_deref(),
        claims.groups.clone().unwrap_or_default(),
    ))
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
