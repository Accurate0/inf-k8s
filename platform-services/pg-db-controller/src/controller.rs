use crate::crd::{Phase, PostgresDatabase, PostgresDatabaseStatus};
use crate::error::{Error, Result};
use crate::sql::{quote_ident, quote_literal};
use base64::{prelude::BASE64_URL_SAFE, Engine};
use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{ObjectMeta, Patch, PatchParams},
    runtime::controller::Action,
    Api, Client, Resource, ResourceExt,
};
use rand::{
    rngs::{StdRng, SysRng},
    Rng, SeedableRng,
};
use sqlx::{AssertSqlSafe, PgPool};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub const FIELD_MANAGER: &str = "pg-db-controller";
const REQUEUE: Duration = Duration::from_secs(3600);
const ERROR_REQUEUE: Duration = Duration::from_secs(30);

pub struct Context {
    pub db: PgPool,
    pub client: Client,
}

pub async fn reconcile(obj: Arc<PostgresDatabase>, ctx: Arc<Context>) -> Result<Action> {
    let ns = obj.namespace().ok_or(Error::MissingNamespace)?;
    let api = Api::<PostgresDatabase>::namespaced(ctx.client.clone(), &ns);
    let name = obj.name_any();
    tracing::info!("reconciling {name}");

    match provision(&obj, &ctx).await {
        Ok(secret_ref) => {
            patch_status(
                &api,
                &name,
                PostgresDatabaseStatus {
                    ready: true,
                    phase: Phase::Ready,
                    observed_generation: obj.meta().generation,
                    message: None,
                    secret_ref: Some(secret_ref),
                },
            )
            .await?;
            Ok(Action::requeue(REQUEUE))
        }
        Err(e) => {
            patch_status(
                &api,
                &name,
                PostgresDatabaseStatus {
                    ready: false,
                    phase: Phase::Error,
                    observed_generation: obj.meta().generation,
                    message: Some(e.to_string()),
                    secret_ref: None,
                },
            )
            .await
            .ok();
            Err(e)
        }
    }
}

pub fn error_policy(_obj: Arc<PostgresDatabase>, err: &Error, _ctx: Arc<Context>) -> Action {
    tracing::error!("reconcile error: {err}");
    Action::requeue(ERROR_REQUEUE)
}

async fn provision(obj: &PostgresDatabase, ctx: &Context) -> Result<String> {
    let database = quote_ident(&obj.spec.database_name)?;
    let role_raw = obj
        .spec
        .role_name
        .as_deref()
        .unwrap_or(&obj.spec.database_name);
    let role = quote_ident(role_raw)?;

    let secrets = Api::<Secret>::namespaced(ctx.client.clone(), &obj.spec.secret_namespace);
    let secret_present = secrets.get_opt(&obj.spec.secret_name).await?.is_some();

    let role_exists = sqlx::query("SELECT 1 FROM pg_roles WHERE rolname = $1")
        .bind(role_raw)
        .fetch_optional(&ctx.db)
        .await?
        .is_some();

    let password = (!secret_present).then(generate_password);

    if !role_exists {
        let pw = password.as_deref().unwrap_or_default();
        sqlx::query(AssertSqlSafe(format!(
            "CREATE ROLE {role} WITH LOGIN PASSWORD {}",
            quote_literal(pw)
        )))
        .execute(&ctx.db)
        .await?;
    } else if let Some(pw) = &password {
        sqlx::query(AssertSqlSafe(format!(
            "ALTER ROLE {role} WITH LOGIN PASSWORD {}",
            quote_literal(pw)
        )))
        .execute(&ctx.db)
        .await?;
    }

    let db_exists = sqlx::query("SELECT 1 FROM pg_database WHERE lower(datname) = lower($1)")
        .bind(&obj.spec.database_name)
        .fetch_optional(&ctx.db)
        .await?
        .is_some();
    if !db_exists {
        sqlx::query(AssertSqlSafe(format!(
            "CREATE DATABASE {database} OWNER {role}"
        )))
        .execute(&ctx.db)
        .await?;
    } else {
        sqlx::query(AssertSqlSafe(format!(
            "ALTER DATABASE {database} OWNER TO {role}"
        )))
        .execute(&ctx.db)
        .await?;
    }

    let secret_ref = format!("{}/{}", obj.spec.secret_namespace, obj.spec.secret_name);

    if let Some(password) = password {
        let secret = build_secret(obj, role_raw, &password, ctx);
        secrets
            .patch(
                &obj.spec.secret_name,
                &PatchParams::apply(FIELD_MANAGER),
                &Patch::Apply(&secret),
            )
            .await?;
        tracing::info!("wrote secret {secret_ref}");
    }

    Ok(secret_ref)
}

fn build_secret(obj: &PostgresDatabase, role: &str, password: &str, ctx: &Context) -> Secret {
    let opts = ctx.db.connect_options();
    let host = opts.get_host();
    let port = opts.get_port();
    let database = &obj.spec.database_name;
    let db_url = format!("postgresql://{role}:{password}@{host}:{port}/{database}");

    let string_data = BTreeMap::from([
        ("PGPASSWORD".to_string(), password.to_string()),
        ("DATABASE_URL".to_string(), db_url),
        ("PGHOST".to_string(), host.to_string()),
        ("PGPORT".to_string(), port.to_string()),
        ("PGDATABASE".to_string(), database.to_string()),
        ("PGUSER".to_string(), role.to_string()),
    ]);

    Secret {
        string_data: Some(string_data),
        immutable: Some(true),
        metadata: ObjectMeta {
            name: Some(obj.spec.secret_name.clone()),
            namespace: Some(obj.spec.secret_namespace.clone()),
            owner_references: Some(vec![obj.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn patch_status(
    api: &Api<PostgresDatabase>,
    name: &str,
    status: PostgresDatabaseStatus,
) -> Result<()> {
    let patch = serde_json::json!({ "status": status });
    api.patch_status(
        name,
        &PatchParams::apply(FIELD_MANAGER),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

fn generate_password() -> String {
    let mut rng = StdRng::try_from_rng(&mut SysRng).expect("system rng unavailable");
    let mut bytes = [0u8; 32];
    rng.fill_bytes(&mut bytes);
    BASE64_URL_SAFE.encode(bytes)
}
