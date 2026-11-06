#![recursion_limit = "256"]

mod access;
mod node;

use hive_router::{
    PluginRegistry, RouterGlobalAllocator, configure_global_allocator, error::RouterInitError,
    init_rustls_crypto_provider, ntex, router_entrypoint,
};

configure_global_allocator!();

#[hive_router::main]
async fn main() -> Result<(), RouterInitError> {
    init_rustls_crypto_provider();

    router_entrypoint(
        PluginRegistry::new()
            .register::<access::AccessPlugin>()
            .register::<node::NodePlugin>(),
    )
    .await
}
