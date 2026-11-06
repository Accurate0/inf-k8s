mod mutation;
mod objects;
mod query;
mod subscription;

use std::sync::Arc;

use async_graphql::{Context, Error, ErrorExtensions, Schema, SchemaBuilder};
use subgraph::node::Ids;
use subgraph::{AuthConfig, Authenticator, Claims};
use tonic::{Code, Request, Status};

use crate::auth::Actor;
use crate::grpc::{AdminService, EvaluationService};
use crate::snapshot::SnapshotManager;
use crate::store::Store;

pub use mutation::MutationRoot;
pub use query::QueryRoot;
pub use subscription::SubscriptionRoot;

pub const IDS: Ids = Ids::new("featureFlags");

/// Graph calls carry no client-id of their own; evaluations are attributed to this.
const CLIENT_ID: &str = "graph";

pub struct Services {
    pub admin: Arc<AdminService>,
    pub evaluation: Arc<EvaluationService>,
    pub store: Store,
    pub mgr: Arc<SnapshotManager>,
}

pub fn builder() -> SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .register_output_type::<objects::Node>()
        .enable_federation()
        .enable_subscription_in_federation()
}

pub fn router(store: Store, mgr: Arc<SnapshotManager>, auth: AuthConfig) -> axum::Router {
    let auth = Authenticator::new(auth);
    tokio::spawn(auth.clone().run());

    let schema = builder()
        .data(Services {
            admin: Arc::new(AdminService::new(store.clone(), mgr.clone())),
            evaluation: Arc::new(EvaluationService::new(mgr.clone())),
            store,
            mgr,
        })
        .finish();
    subgraph::router(schema, auth)
}

/// A gRPC request as the AuthLayer would have built it for this user.
fn request<T>(ctx: &Context<'_>, message: T) -> async_graphql::Result<Request<T>> {
    let claims = ctx.data::<Claims>()?;
    let mut request = Request::new(message);
    request
        .extensions_mut()
        .insert(Actor(claims.identity().to_owned()));
    request
        .metadata_mut()
        .insert("client-id", CLIENT_ID.parse().expect("ascii"));
    Ok(request)
}

fn grpc(status: Status) -> Error {
    let code = match status.code() {
        Code::NotFound => "NOT_FOUND",
        Code::InvalidArgument => "BAD_USER_INPUT",
        Code::Aborted | Code::FailedPrecondition | Code::AlreadyExists => "CONFLICT",
        Code::Unauthenticated => "UNAUTHENTICATED",
        Code::PermissionDenied => "FORBIDDEN",
        _ => "INTERNAL_SERVER_ERROR",
    };
    let message = if code == "INTERNAL_SERVER_ERROR" {
        tracing::error!("graphql request failed: {status}");
        "internal error".to_owned()
    } else {
        status.message().to_owned()
    };
    Error::new(message).extend_with(|_, ext| ext.set("code", code))
}

#[cfg(test)]
mod tests {
    #[test]
    fn subgraph_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("subgraph.graphql");
        subgraph::assert_subgraph_current(&super::builder().finish(), path);
    }
}
