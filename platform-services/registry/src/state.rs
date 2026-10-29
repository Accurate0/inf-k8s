use sqlx::PgPool;

use crate::{auth::Auth, config::Config, storage::Storage};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: PgPool,
    pub storage: Storage,
    pub auth: Auth,
}

impl AppState {
    pub fn new(config: Config, pool: PgPool, storage: Storage) -> Self {
        Self {
            auth: Auth::from_config(&config),
            config,
            pool,
            storage,
        }
    }
}
