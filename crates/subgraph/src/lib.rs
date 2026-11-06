pub mod auth;
pub mod node;
pub mod server;

pub use async_graphql;
pub use async_graphql_axum;

pub use auth::{AuthConfig, AuthError, Authenticator, Claims, Provider};
pub use server::{GRAPHQL_PATH, WEBSOCKET_PATH, router};

/// Rewrites the committed federation SDL on drift and fails so the change gets committed.
pub fn assert_subgraph_current<Q, M, S>(
    schema: &async_graphql::Schema<Q, M, S>,
    path: impl AsRef<std::path::Path>,
) where
    Q: async_graphql::ObjectType + 'static,
    M: async_graphql::ObjectType + 'static,
    S: async_graphql::SubscriptionType + 'static,
{
    let sdl = schema.sdl_with_options(async_graphql::SDLExportOptions::new().federation());
    let path = path.as_ref();
    let current = std::fs::read_to_string(path).unwrap_or_default();

    if current != sdl {
        std::fs::write(path, &sdl).expect("write subgraph.graphql");
        panic!(
            "{} was out of date and has been regenerated, commit it",
            path.display()
        );
    }
}
