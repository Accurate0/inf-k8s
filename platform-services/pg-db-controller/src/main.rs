use futures::StreamExt;
use kube::{
    runtime::{controller::Controller, watcher},
    Api, Client,
};
use pg_db_controller::controller::{error_policy, reconcile, Context};
use pg_db_controller::{PostgresDatabase, Result};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    tracing::info!("connected to postgres");

    let client = Client::try_default().await?;
    let api = Api::<PostgresDatabase>::all(client.clone());
    let ctx = Arc::new(Context { db: pool, client });

    Controller::new(api, watcher::Config::default())
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!("reconcile failed: {e}");
            }
        })
        .await;

    Ok(())
}
