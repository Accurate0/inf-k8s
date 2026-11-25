//! Serves the generated `me` subgraph inside the router: who the verified token says the caller is,
//! the scopes it holds and the subgraphs it may use, as the access plugin worked them out.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_graphql::Value;
use async_graphql::dynamic::{Enum, Field, FieldFuture, FieldValue, Object, Schema, TypeRef};
use hive_router::{
    async_trait,
    plugins::{
        hooks::{
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
            on_subgraph_execute::{
                OnSubgraphExecuteStartHookPayload, OnSubgraphExecuteStartHookResult,
            },
        },
        plugin_trait::{RouterPlugin, StartHookPayload},
    },
    tracing,
};
use moka::future::Cache;
use serde::Deserialize;

use crate::access::{CallerKind, CallerSummary};
use crate::node::respond;

#[derive(Deserialize)]
pub struct MeConfig {
    subgraph: String,
}

pub struct MePlugin {
    subgraph: String,
    schema: Schema,
    userinfo: Cache<String, Arc<UserInfo>>,
    client: reqwest::Client,
}

#[derive(Clone, Default, Deserialize)]
struct UserInfo {
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
}

struct Me {
    caller: CallerSummary,
    info: Arc<UserInfo>,
}

#[async_trait]
impl RouterPlugin for MePlugin {
    type Config = MeConfig;

    fn plugin_name() -> &'static str {
        "me"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let config = payload.config()?;
        payload.initialize_plugin(Self {
            subgraph: config.subgraph,
            schema: schema().map_err(|error| error.to_string())?,
            userinfo: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(300))
                .build(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|error| error.to_string())?,
        })
    }

    async fn on_subgraph_execute<'exec>(
        &'exec self,
        payload: OnSubgraphExecuteStartHookPayload<'exec>,
    ) -> OnSubgraphExecuteStartHookResult<'exec> {
        if payload.subgraph_name != self.subgraph {
            return payload.proceed();
        }

        let caller = payload
            .context
            .get_ref::<CallerSummary>()
            .map(|caller| caller.clone());
        let me = match caller {
            Some(caller) => {
                let info = match caller.kind {
                    CallerKind::User => self.userinfo(&caller).await,
                    CallerKind::External(_) => Arc::default(),
                };
                Some(Me { caller, info })
            }
            None => None,
        };

        let response = respond(
            &self.schema,
            &payload.execution_request,
            "ME_PLUGIN",
            |r| match me {
                Some(me) => r.data(Arc::new(me)),
                None => r,
            },
        )
        .await;
        payload.end_with_response(response)
    }
}

impl MePlugin {
    async fn userinfo(&self, caller: &CallerSummary) -> Arc<UserInfo> {
        let url = format!("{}/userinfo", caller.issuer.trim_end_matches('/'));
        self.userinfo
            .try_get_with(caller.token.clone(), async {
                self.client
                    .get(&url)
                    .bearer_auth(&caller.token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<UserInfo>()
                    .await
                    .map(Arc::new)
            })
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(issuer = caller.issuer, "userinfo lookup failed: {error}");
                Arc::default()
            })
    }
}

fn me<'a>(ctx: &async_graphql::dynamic::ResolverContext<'a>) -> async_graphql::Result<&'a Me> {
    Ok(ctx.parent_value.try_downcast_ref::<Arc<Me>>()?.as_ref())
}

fn text(name: &str, ty: TypeRef, get: fn(&Me) -> Option<String>) -> Field {
    Field::new(name, ty, move |ctx| {
        FieldFuture::new(async move { Ok(get(me(&ctx)?).map(FieldValue::value)) })
    })
}

fn list(name: &str, get: fn(&Me) -> Vec<String>) -> Field {
    Field::new(
        name,
        TypeRef::named_nn_list_nn(TypeRef::STRING),
        move |ctx| {
            FieldFuture::new(async move {
                let values = get(me(&ctx)?)
                    .into_iter()
                    .map(Value::from)
                    .collect::<Vec<_>>();
                Ok(Some(FieldValue::value(Value::List(values))))
            })
        },
    )
}

fn expires_in(me: &Me) -> Option<i64> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    me.caller.expires_at.map(|exp| (exp - now).max(0))
}

fn schema() -> Result<Schema, async_graphql::dynamic::SchemaError> {
    let query = Object::new("Query").field(Field::new("me", TypeRef::named("Me"), |ctx| {
        FieldFuture::new(async move {
            Ok(ctx
                .data_opt::<Arc<Me>>()
                .map(|me| FieldValue::owned_any(me.clone())))
        })
    }));

    let me_type = Object::new("Me")
        .field(text("subject", TypeRef::named_nn(TypeRef::STRING), |me| {
            Some(me.caller.subject.clone())
        }))
        .field(text("username", TypeRef::named(TypeRef::STRING), |me| {
            me.caller
                .username
                .clone()
                .or_else(|| me.info.preferred_username.clone())
        }))
        .field(text("email", TypeRef::named(TypeRef::STRING), |me| {
            me.info.email.clone()
        }))
        .field(Field::new("kind", TypeRef::named_nn("MeKind"), |ctx| {
            FieldFuture::new(async move {
                let kind = match me(&ctx)?.caller.kind {
                    CallerKind::User => "USER",
                    CallerKind::External(_) => "EXTERNAL",
                };
                Ok(Some(FieldValue::value(Value::Enum(
                    async_graphql::Name::new(kind),
                ))))
            })
        }))
        .field(text("issuer", TypeRef::named_nn(TypeRef::STRING), |me| {
            Some(me.caller.issuer.clone())
        }))
        .field(text(
            "externalIssuer",
            TypeRef::named(TypeRef::STRING),
            |me| match &me.caller.kind {
                CallerKind::External(name) => Some(name.clone()),
                CallerKind::User => None,
            },
        ))
        .field(text("client", TypeRef::named(TypeRef::STRING), |me| {
            me.caller.client.clone()
        }))
        .field(list("scopes", |me| {
            me.caller.scopes.iter().cloned().collect()
        }))
        .field(list("groups", |me| {
            me.info
                .groups
                .iter()
                .filter(|group| group.contains('@'))
                .cloned()
                .collect()
        }))
        .field(list("subgraphs", |me| me.caller.subgraphs.clone()))
        .field(Field::new(
            "expiresIn",
            TypeRef::named(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    Ok(expires_in(me(&ctx)?).map(|s| FieldValue::value(Value::from(s))))
                })
            },
        ));

    Schema::build("Query", None, None)
        .register(query)
        .register(me_type)
        .register(Enum::new("MeKind").item("USER").item("EXTERNAL"))
        .finish()
}
