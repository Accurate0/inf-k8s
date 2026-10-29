use std::sync::Arc;

use anyhow::Context;
use p256::ecdsa::{SigningKey, VerifyingKey};
use sqlx::PgPool;

use crate::{auth::Auth, config::Config, storage::Storage};

#[derive(Clone)]
pub struct Signer {
    key: Arc<SigningKey>,
}

impl Signer {
    pub fn new(key: SigningKey) -> Self {
        Self { key: Arc::new(key) }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let pem = std::env::var("SIGNING_KEY").context("SIGNING_KEY must be set")?;
        let key = registry_api::signing::signing_key_from_pem(&pem).map_err(anyhow::Error::msg)?;
        Ok(Self::new(key))
    }

    pub fn key(&self) -> &SigningKey {
        &self.key
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        *self.key.verifying_key()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: PgPool,
    pub storage: Storage,
    pub auth: Auth,
    pub signer: Signer,
}

impl AppState {
    pub fn new(config: Config, pool: PgPool, storage: Storage, signer: Signer) -> Self {
        Self {
            auth: Auth::from_config(&config),
            config,
            pool,
            storage,
            signer,
        }
    }
}
