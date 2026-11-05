//! Serves the generated `node` subgraph inside the router: `Query.node` turns a global id (`<service>:<node>:<key>`) into a `{__typename, id}` reference, and the planner
//! fetches the rest from the owning subgraph by `@key(fields: "id")`.

use async_graphql::dynamic::{
    Field, FieldFuture, FieldValue, InputValue, Interface, InterfaceField, Object, Schema, TypeRef,
};
use async_graphql::{ID, Request, Variables};
use bytes::Bytes;
use hive_router::{
    GraphQLError, async_trait,
    executor::{
        executors::common::SubgraphExecutionRequest, response::subgraph_response::SubgraphResponse,
    },
    plugins::{
        hooks::{
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
            on_subgraph_execute::{
                OnSubgraphExecuteStartHookPayload, OnSubgraphExecuteStartHookResult,
            },
        },
        plugin_trait::{RouterPlugin, StartHookPayload},
    },
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NodeConfig {
    subgraph: String,
    types: Vec<String>,
}

pub struct NodePlugin {
    subgraph: String,
    schema: Schema,
}

#[async_trait]
impl RouterPlugin for NodePlugin {
    type Config = NodeConfig;

    fn plugin_name() -> &'static str {
        "node"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let config = payload.config()?;
        let schema = schema(&config.types)?;
        payload.initialize_plugin(Self {
            subgraph: config.subgraph,
            schema,
        })
    }

    async fn on_subgraph_execute<'exec>(
        &'exec self,
        payload: OnSubgraphExecuteStartHookPayload<'exec>,
    ) -> OnSubgraphExecuteStartHookResult<'exec> {
        if payload.subgraph_name != self.subgraph {
            return payload.proceed();
        }

        let response = respond(&self.schema, &payload.execution_request).await;
        payload.end_with_response(response)
    }
}

async fn respond(
    schema: &Schema,
    request: &SubgraphExecutionRequest<'_>,
) -> SubgraphResponse<'static> {
    let failed = |message: String| SubgraphResponse {
        errors: Some(vec![GraphQLError::from_message_and_code(
            message,
            "NODE_PLUGIN",
        )]),
        ..Default::default()
    };

    let variables = match request
        .variables
        .iter()
        .flatten()
        .map(|(name, value)| {
            let value = serde_json::from_str(&hive_router::sonic_rs::to_string(value)?)?;
            Ok::<_, Box<dyn std::error::Error>>((name.to_string(), value))
        })
        .collect::<Result<serde_json::Map<_, _>, _>>()
    {
        Ok(variables) => variables,
        Err(error) => return failed(format!("invalid variables: {error}")),
    };

    let mut graphql = Request::new(request.query)
        .variables(Variables::from_json(serde_json::Value::Object(variables)));
    if let Some(name) = &request.operation_name {
        graphql = graphql.operation_name(name);
    }

    let body = match serde_json::to_vec(&schema.execute(graphql).await) {
        Ok(body) => body,
        Err(error) => return failed(error.to_string()),
    };
    SubgraphResponse::deserialize_from_bytes(Bytes::from(body), request.custom_scalar_paths)
        .unwrap_or_else(|error| failed(error.to_string()))
}

/// `spotify:exportFile:42` names `SpotifyExportFile`, when that type implements Node.
fn typename<'t>(id: &str, types: &'t [String]) -> Option<&'t str> {
    let mut parts = id.splitn(3, ':');
    let (service, node, key) = (parts.next()?, parts.next()?, parts.next()?);
    if service.is_empty() || node.is_empty() || key.is_empty() {
        return None;
    }

    let name = format!("{}{}", pascal(service), pascal(node));
    types.iter().find(|ty| **ty == name).map(String::as_str)
}

fn pascal(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn reference(id: ID, types: &[String]) -> Option<FieldValue<'static>> {
    let ty = typename(&id, types)?.to_owned();
    Some(FieldValue::owned_any(id).with_type(ty))
}

fn schema(types: &[String]) -> Result<Schema, async_graphql::dynamic::SchemaError> {
    let node_types = types.to_vec();

    let query = Object::new("Query").field(
        Field::new("node", TypeRef::named("Node"), move |ctx| {
            let types = node_types.clone();
            FieldFuture::new(async move {
                let id = ID(ctx.args.try_get("id")?.string()?.to_owned());
                Ok(reference(id, &types))
            })
        })
        .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID))),
    );

    let mut builder = Schema::build("Query", None, None).register(query).register(
        Interface::new("Node").field(InterfaceField::new("id", TypeRef::named_nn(TypeRef::ID))),
    );

    for ty in types {
        builder = builder.register(Object::new(ty).implement("Node").field(Field::new(
            "id",
            TypeRef::named_nn(TypeRef::ID),
            |ctx| {
                FieldFuture::new(async move {
                    let id = ctx.parent_value.try_downcast_ref::<ID>()?;
                    Ok(Some(FieldValue::value(id.to_string())))
                })
            },
        )));
    }

    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types() -> Vec<String> {
        vec![
            "SpotifyArtist".into(),
            "SpotifyExportFile".into(),
            "SpotifyItem".into(),
        ]
    }

    #[test]
    fn ids_name_their_type() {
        let types = types();
        assert_eq!(
            typename("spotify:artist:Radiohead", &types),
            Some("SpotifyArtist")
        );
        assert_eq!(
            typename("spotify:exportFile:42", &types),
            Some("SpotifyExportFile")
        );
        assert_eq!(
            typename("spotify:item:spotify:track:4uLU6h", &types),
            Some("SpotifyItem")
        );
        assert_eq!(typename("spotify:album:x", &types), None);
        assert_eq!(typename("home:artist:x", &types), None);
        assert_eq!(typename("spotify:artist:", &types), None);
        assert_eq!(typename("spotify:artist", &types), None);
    }

    #[tokio::test]
    async fn resolves_references_the_planner_asks_for() {
        let schema = schema(&types()).unwrap();
        let response = schema
            .execute(
                Request::new(
                    r#"query($id: ID!, $skip: Boolean!) {
                        a: node(id: $id) { __typename id ... on SpotifyArtist { id } }
                        item: node(id: "spotify:item:spotify:track:1") { __typename ... on Node { id } }
                        missing: node(id: "spotify:album:x") { id }
                        skipped: node(id: $id) @skip(if: $skip) { id }
                    }"#,
                )
                .variables(Variables::from_json(serde_json::json!({
                    "id": "spotify:artist:Radiohead",
                    "skip": true,
                }))),
            )
            .await;

        assert!(response.errors.is_empty(), "{:?}", response.errors);
        assert_eq!(
            response.data.into_json().unwrap(),
            serde_json::json!({
                "a": {"__typename": "SpotifyArtist", "id": "spotify:artist:Radiohead"},
                "item": {"__typename": "SpotifyItem", "id": "spotify:item:spotify:track:1"},
                "missing": null,
            })
        );
    }
}
