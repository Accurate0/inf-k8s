use crate::crd::{Condition, PostgresDatabase};
use crate::error::{Error, Result};
use crate::sql::{is_valid_ident, quote_ident, quote_literal};
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
const PROGRAMMED_OK: &str = "database, role, and credentials secret provisioned";
const REQUEUE: Duration = Duration::from_secs(3600);
const ERROR_REQUEUE: Duration = Duration::from_secs(30);

pub struct Context {
    pub db: PgPool,
    pub client: Client,
}

pub async fn reconcile(obj: Arc<PostgresDatabase>, ctx: Arc<Context>) -> Result<Action> {
    let accepted = validate(&obj);
    let provisioned = match &accepted {
        Ok(()) => {
            tracing::info!("reconciling {}", obj.name_any());
            provision(&obj, &ctx).await
        }
        Err(_) => Ok(()),
    };

    if let Err(e) = write_status(&obj, &ctx, &accepted, provisioned.as_ref().err()).await {
        tracing::warn!("failed to write status for {}: {e}", obj.name_any());
    }

    accepted.map_err(Error::Validation)?;
    provisioned?;

    Ok(Action::requeue(REQUEUE))
}

pub fn error_policy(_obj: Arc<PostgresDatabase>, err: &Error, _ctx: Arc<Context>) -> Action {
    tracing::error!("reconcile error: {err}");
    Action::requeue(ERROR_REQUEUE)
}

fn validate(obj: &PostgresDatabase) -> Result<(), String> {
    if !is_valid_ident(&obj.spec.database_name) {
        return Err(format!("invalid databaseName: {:?}", obj.spec.database_name));
    }
    if let Some(role) = &obj.spec.role_name {
        if !is_valid_ident(role) {
            return Err(format!("invalid roleName: {role:?}"));
        }
    }
    Ok(())
}

fn condition(
    existing: Option<&Vec<Condition>>,
    type_: &str,
    status: &str,
    reason: &str,
    message: &str,
    observed_generation: i64,
) -> Condition {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let last_transition_time = existing
        .and_then(|conds| conds.iter().find(|c| c.type_ == type_))
        .filter(|c| c.status == status)
        .map(|c| c.last_transition_time.clone())
        .unwrap_or(now);

    Condition {
        type_: type_.to_string(),
        status: status.to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        observed_generation,
        last_transition_time,
    }
}

async fn write_status(
    obj: &PostgresDatabase,
    ctx: &Context,
    accepted: &Result<(), String>,
    provision_error: Option<&Error>,
) -> Result<()> {
    let namespace = obj.namespace().ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let generation = obj.meta().generation.unwrap_or(0);
    let existing = obj.status.as_ref().map(|s| &s.conditions);

    let (accepted_status, accepted_reason, accepted_message) = match accepted {
        Ok(()) => ("True", "Accepted", "PostgresDatabase has been accepted".to_string()),
        Err(msg) => ("False", "InvalidSpec", msg.clone()),
    };

    let (programmed_status, programmed_reason, programmed_message) = match accepted {
        Err(_) => ("False", "NotAccepted", "spec validation failed".to_string()),
        Ok(()) => match provision_error {
            None => ("True", "Programmed", PROGRAMMED_OK.to_string()),
            Some(e) => ("False", "ReconcileFailed", e.to_string()),
        },
    };

    let conditions = vec![
        condition(existing, "Accepted", accepted_status, accepted_reason, &accepted_message, generation),
        condition(existing, "Programmed", programmed_status, programmed_reason, &programmed_message, generation),
    ];

    let patch = serde_json::json!({ "status": { "conditions": conditions } });
    Api::<PostgresDatabase>::namespaced(ctx.client.clone(), &namespace)
        .patch_status(&obj.name_any(), &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(())
}

async fn provision(obj: &PostgresDatabase, ctx: &Context) -> Result<()> {
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

    if let Some(password) = password {
        let secret = build_secret(obj, role_raw, &password, ctx);
        secrets
            .patch(
                &obj.spec.secret_name,
                &PatchParams::apply(FIELD_MANAGER),
                &Patch::Apply(&secret),
            )
            .await?;
        tracing::info!("wrote secret {}/{}", obj.spec.secret_namespace, obj.spec.secret_name);
    }

    Ok(())
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

fn generate_password() -> String {
    let mut rng = StdRng::try_from_rng(&mut SysRng).expect("system rng unavailable");
    let mut bytes = [0u8; 32];
    rng.fill_bytes(&mut bytes);
    BASE64_URL_SAFE.encode(bytes)
}
