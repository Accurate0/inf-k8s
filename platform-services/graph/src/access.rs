//! Gates subgraphs on Kanidm groups at the router: operations planned against a subgraph the caller
//! can't use are rejected before any fetch, and introspection hides what only those subgraphs serve.
//! Each subgraph still authorizes on its own.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hive_router::{
    ArcSwap, DashMap, GraphQLError, async_trait,
    executor::{execution::client_request_details::JwtRequestDetails, response::value::Value},
    graphql_tools::{
        parser::schema::{Definition, Directive, TypeDefinition},
        static_graphql::schema::Document,
    },
    http::StatusCode,
    plugins::{
        hooks::{
            on_execute::{OnExecuteStartHookPayload, OnExecuteStartHookResult},
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
            on_query_plan::{OnQueryPlanStartHookPayload, OnQueryPlanStartHookResult},
            on_supergraph_load::{
                OnSupergraphLoadStartHookPayload, OnSupergraphLoadStartHookResult,
            },
        },
        plugin_trait::{EndHookPayload, RouterPlugin, StartHookPayload},
    },
    query_planner::planner::plan_nodes::{PlanNode, QueryPlan},
    sonic_rs::{JsonContainerTrait, JsonValueTrait},
    tracing,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AccessConfig {
    subgraphs: HashMap<String, Gate>,
}

#[derive(Deserialize)]
struct Gate {
    issuers: Vec<String>,
    groups: Vec<String>,
}

impl Gate {
    fn admits(&self, issuer: Option<&str>, held: &[String]) -> bool {
        issuer.is_some_and(|iss| self.issuers.iter().any(|i| i == iss))
            && groups_match(held, &self.groups)
    }
}

pub struct AccessPlugin {
    subgraphs: HashMap<String, Gate>,
    ownership: ArcSwap<Ownership>,
    groups: DashMap<String, (Arc<Vec<String>>, u64)>,
    client: reqwest::Client,
}

/// Gated subgraphs whose clients didn't issue the caller's token, or whose groups it holds none of.
#[derive(Clone, Default)]
struct Denied(Arc<HashSet<String>>);

#[derive(Default)]
struct Ownership {
    types: HashMap<String, Vec<String>>,
    fields: HashMap<String, HashMap<String, Vec<String>>>,
}

#[async_trait]
impl RouterPlugin for AccessPlugin {
    type Config = AccessConfig;

    fn plugin_name() -> &'static str {
        "access"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let config = payload.config()?;
        payload.initialize_plugin(Self {
            subgraphs: config.subgraphs,
            ownership: ArcSwap::default(),
            groups: DashMap::new(),
            client: reqwest::Client::new(),
        })
    }

    fn on_supergraph_reload<'exec>(
        &'exec self,
        payload: OnSupergraphLoadStartHookPayload,
    ) -> OnSupergraphLoadStartHookResult<'exec> {
        self.ownership
            .store(Arc::new(Ownership::from(&payload.new_ast)));
        payload.proceed()
    }

    async fn on_graphql_analysis<'exec>(
        &'exec self,
        payload: &mut OnGraphqlAnalysisHookPayload<'exec>,
    ) -> OnGraphqlAnalysisHookResult {
        if self.subgraphs.is_empty() {
            return OnGraphqlAnalysisHookResult::Proceed;
        }

        let (issuer, held) = match payload.client_request_details.jwt.as_ref() {
            JwtRequestDetails::Authenticated { token, claims, .. } => (
                claims.get("iss").and_then(|iss| iss.as_str()),
                self.held_groups(token, claims).await,
            ),
            JwtRequestDetails::Unauthenticated => (None, Arc::default()),
        };

        let denied = self
            .subgraphs
            .iter()
            .filter(|(_, gate)| !gate.admits(issuer, &held))
            .map(|(name, _)| name.clone())
            .collect();
        payload.context.insert(Denied(Arc::new(denied)));

        OnGraphqlAnalysisHookResult::Proceed
    }

    async fn on_query_plan<'exec>(
        &'exec self,
        payload: OnQueryPlanStartHookPayload<'exec>,
    ) -> OnQueryPlanStartHookResult<'exec> {
        let denied = self.denied(
            payload
                .context
                .get_ref::<Denied>()
                .map(|d| d.as_ref().clone()),
        );
        if denied.0.is_empty() {
            return payload.proceed();
        }

        payload.on_end(
            move |end| match forbidden(end.query_plan(), &denied.0).map(str::to_owned) {
                Some(subgraph) => end.end_with_graphql_error(
                    GraphQLError::from_message_and_code(
                        format!("not allowed to query the {subgraph} subgraph"),
                        "FORBIDDEN",
                    ),
                    StatusCode::FORBIDDEN,
                ),
                None => end.proceed(),
            },
        )
    }

    async fn on_execute<'exec>(
        &'exec self,
        mut payload: OnExecuteStartHookPayload<'exec>,
    ) -> OnExecuteStartHookResult<'exec> {
        let denied = self.denied(
            payload
                .context
                .get_ref::<Denied>()
                .map(|d| d.as_ref().clone()),
        );
        if !denied.0.is_empty() {
            let hidden = Hidden {
                ownership: &self.ownership.load(),
                denied: &denied.0,
            };
            hidden.prune_response(&mut payload.data);
        }

        payload.proceed()
    }
}

impl AccessPlugin {
    /// No context means analysis never ran for this request, so nothing gated is allowed.
    fn denied(&self, context: Option<Denied>) -> Denied {
        context.unwrap_or_else(|| Denied(Arc::new(self.subgraphs.keys().cloned().collect())))
    }

    async fn held_groups(
        &self,
        token: &str,
        claims: &hive_router::sonic_rs::Value,
    ) -> Arc<Vec<String>> {
        if let Some(groups) = claims.get("groups").and_then(|g| g.as_array()) {
            return Arc::new(
                groups
                    .iter()
                    .filter_map(|g| g.as_str().map(String::from))
                    .collect(),
            );
        }

        let now = now();
        if let Some(cached) = self.groups.get(token).filter(|entry| entry.1 > now) {
            return cached.0.clone();
        }

        let Some(issuer) = claims.get("iss").and_then(|iss| iss.as_str()) else {
            return Arc::default();
        };
        let groups = match self.userinfo(issuer, token).await {
            Ok(groups) => Arc::new(groups),
            Err(error) => {
                tracing::warn!(issuer, "userinfo lookup failed: {error}");
                return Arc::default();
            }
        };

        if let Some(exp) = claims.get("exp").and_then(|exp| exp.as_u64()) {
            self.groups.retain(|_, entry| entry.1 > now);
            self.groups.insert(token.to_owned(), (groups.clone(), exp));
        }
        groups
    }

    async fn userinfo(&self, issuer: &str, token: &str) -> Result<Vec<String>, reqwest::Error> {
        #[derive(Deserialize)]
        struct UserInfo {
            #[serde(default)]
            groups: Vec<String>,
        }

        let info: UserInfo = self
            .client
            .get(format!("{}/userinfo", issuer.trim_end_matches('/')))
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(info.groups)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Kanidm sends groups as SPNs (`name@domain`) and uuids; either the SPN or its short name matches.
fn groups_match(held: &[String], allowed: &[String]) -> bool {
    held.iter().any(|group| {
        let name = group
            .split_once('@')
            .map_or(group.as_str(), |(name, _)| name);
        allowed.iter().any(|a| a == group || a == name)
    })
}

fn forbidden<'p>(plan: &'p QueryPlan, denied: &HashSet<String>) -> Option<&'p str> {
    fn walk<'p>(node: &'p PlanNode, denied: &HashSet<String>) -> Option<&'p str> {
        let service = |name: &'p String| denied.contains(name).then_some(name.as_str());
        match node {
            PlanNode::Fetch(fetch) => service(&fetch.service_name),
            PlanNode::BatchFetch(fetch) => service(&fetch.service_name),
            PlanNode::Subscription(subscription) => service(&subscription.primary.service_name),
            PlanNode::Flatten(flatten) => walk(&flatten.node, denied),
            PlanNode::Sequence(sequence) => sequence.nodes.iter().find_map(|n| walk(n, denied)),
            PlanNode::Parallel(parallel) => parallel.nodes.iter().find_map(|n| walk(n, denied)),
            PlanNode::Defer(defer) => defer
                .primary
                .node
                .iter()
                .chain(defer.deferred.iter().flat_map(|d| d.node.iter()))
                .find_map(|n| walk(n, denied)),
        }
    }

    plan.node.as_ref().and_then(|node| walk(node, denied))
}

impl From<&Document> for Ownership {
    fn from(document: &Document) -> Self {
        let mut graphs = HashMap::new();
        for definition in &document.definitions {
            if let Definition::TypeDefinition(TypeDefinition::Enum(e)) = definition
                && e.name == "join__Graph"
            {
                for value in &e.values {
                    if let Some(name) = value
                        .directives
                        .iter()
                        .find(|d| d.name == "join__graph")
                        .and_then(|d| argument(d, "name"))
                    {
                        graphs.insert(value.name.clone(), name.to_owned());
                    }
                }
            }
        }

        let joined = |directives: &[Directive<'static, String>], name: &str| -> Vec<String> {
            let all: Vec<_> = directives.iter().filter(|d| d.name == name).collect();
            let resolving: Vec<_> = all
                .iter()
                .filter(|d| !flag(d, "external") && argument(d, "resolvable") != Some("false"))
                .collect();
            let chosen = if resolving.is_empty() {
                all.iter().collect()
            } else {
                resolving
            };
            chosen
                .into_iter()
                .filter_map(|d| argument(d, "graph"))
                .filter_map(|graph| graphs.get(graph).cloned())
                .collect()
        };

        let mut ownership = Self::default();
        for definition in &document.definitions {
            let Definition::TypeDefinition(definition) = definition else {
                continue;
            };
            let (name, directives, fields): (_, _, Vec<_>) = match definition {
                TypeDefinition::Object(t) => (
                    &t.name,
                    &t.directives,
                    t.fields.iter().map(|f| (&f.name, &f.directives)).collect(),
                ),
                TypeDefinition::Interface(t) => (
                    &t.name,
                    &t.directives,
                    t.fields.iter().map(|f| (&f.name, &f.directives)).collect(),
                ),
                TypeDefinition::InputObject(t) => (
                    &t.name,
                    &t.directives,
                    t.fields.iter().map(|f| (&f.name, &f.directives)).collect(),
                ),
                TypeDefinition::Enum(t) => (&t.name, &t.directives, vec![]),
                TypeDefinition::Union(t) => (&t.name, &t.directives, vec![]),
                TypeDefinition::Scalar(t) => (&t.name, &t.directives, vec![]),
            };

            ownership
                .types
                .insert(name.clone(), joined(directives, "join__type"));
            ownership.fields.insert(
                name.clone(),
                fields
                    .into_iter()
                    .map(|(field, directives)| (field.clone(), joined(directives, "join__field")))
                    .filter(|(_, graphs)| !graphs.is_empty())
                    .collect(),
            );
        }
        ownership
    }
}

fn argument<'d>(directive: &'d Directive<'static, String>, name: &str) -> Option<&'d str> {
    use hive_router::graphql_tools::parser::query::Value as Arg;
    directive
        .arguments
        .iter()
        .find(|(arg, _)| arg == name)
        .and_then(|(_, value)| match value {
            Arg::Enum(v) | Arg::String(v) => Some(v.as_str()),
            Arg::Boolean(false) => Some("false"),
            _ => None,
        })
}

fn flag(directive: &Directive<'static, String>, name: &str) -> bool {
    use hive_router::graphql_tools::parser::query::Value as Arg;
    directive
        .arguments
        .iter()
        .any(|(arg, value)| arg == name && matches!(value, Arg::Boolean(true)))
}

struct Hidden<'h> {
    ownership: &'h Ownership,
    denied: &'h HashSet<String>,
}

impl Hidden<'_> {
    fn graphs_denied(&self, graphs: &[String]) -> bool {
        !graphs.is_empty() && graphs.iter().all(|g| self.denied.contains(g))
    }

    fn type_hidden(&self, name: &str) -> bool {
        self.ownership
            .types
            .get(name)
            .is_some_and(|graphs| self.graphs_denied(graphs))
    }

    fn field_hidden(&self, owner: Option<&str>, field: &Value) -> bool {
        let own = owner
            .zip(str_of(field, "name"))
            .is_some_and(|(owner, name)| {
                self.ownership
                    .fields
                    .get(owner)
                    .and_then(|fields| fields.get(name))
                    .is_some_and(|graphs| self.graphs_denied(graphs))
            });
        let returns = get(field, "type")
            .and_then(named)
            .is_some_and(|t| self.type_hidden(t));
        let args = match get(field, "args") {
            Some(Value::Array(args)) => args.iter().any(|arg| {
                get(arg, "type")
                    .and_then(named)
                    .is_some_and(|t| self.type_hidden(t))
            }),
            _ => false,
        };
        own || returns || args
    }

    fn prune_response(&self, data: &mut Value) {
        let Value::Object(entries) = data else {
            return;
        };
        for (key, value) in entries.iter_mut() {
            match *key {
                "__schema" => self.prune_schema(value),
                "__type" => {
                    if str_of(value, "name").is_some_and(|n| self.type_hidden(n)) {
                        *value = Value::Null;
                    } else {
                        self.prune_type(value);
                    }
                }
                _ => {}
            }
        }
    }

    fn prune_schema(&self, schema: &mut Value) {
        let Value::Object(entries) = schema else {
            return;
        };
        for (key, value) in entries.iter_mut() {
            match *key {
                "types" => {
                    if let Value::Array(types) = value {
                        types.retain(|t| !str_of(t, "name").is_some_and(|n| self.type_hidden(n)));
                        types.iter_mut().for_each(|t| self.prune_type(t));
                    }
                }
                "queryType" | "mutationType" | "subscriptionType"
                    if str_of(value, "name").is_some_and(|n| self.type_hidden(n)) =>
                {
                    *value = Value::Null;
                }
                _ => {}
            }
        }
    }

    fn prune_type(&self, ty: &mut Value) {
        let owner = str_of(ty, "name").map(str::to_owned);
        let Value::Object(entries) = ty else {
            return;
        };
        for (key, value) in entries.iter_mut() {
            let Value::Array(items) = value else {
                continue;
            };
            match *key {
                "fields" | "inputFields" => {
                    items.retain(|f| !self.field_hidden(owner.as_deref(), f))
                }
                "interfaces" | "possibleTypes" => {
                    items.retain(|t| !str_of(t, "name").is_some_and(|n| self.type_hidden(n)))
                }
                _ => {}
            }
        }
    }
}

fn get<'v, 'a>(value: &'v Value<'a>, key: &str) -> Option<&'v Value<'a>> {
    match value {
        Value::Object(entries) => entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn str_of<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    match get(value, key)? {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// The named type under any NON_NULL/LIST wrappers of an introspection `__Type`.
fn named<'v>(ty: &'v Value) -> Option<&'v str> {
    str_of(ty, "name").or_else(|| get(ty, "ofType").and_then(named))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_router::graphql_tools::parser::parse_schema;

    const SUPERGRAPH: &str = r#"
        enum join__Graph {
          NODE @join__graph(name: "node", url: "http://node")
          SPOTIFY @join__graph(name: "spotify", url: "http://spotify")
          MACCAS @join__graph(name: "maccas", url: "http://maccas")
        }
        type Query @join__type(graph: NODE) @join__type(graph: SPOTIFY) @join__type(graph: MACCAS) {
          node(id: ID!): Node @join__field(graph: NODE)
          spotifyAccount(filter: SpotifyFilter): SpotifyAccount @join__field(graph: SPOTIFY)
          maccasDeals: [String!]! @join__field(graph: MACCAS)
        }
        type Mutation @join__type(graph: SPOTIFY) {
          spotifyLink: Boolean! @join__field(graph: SPOTIFY)
        }
        interface Node @join__type(graph: NODE) @join__type(graph: SPOTIFY) {
          id: ID!
        }
        type SpotifyAccount implements Node @join__type(graph: NODE, key: "id", resolvable: false) @join__type(graph: SPOTIFY, key: "id") {
          id: ID!
          slug: String! @join__field(graph: SPOTIFY)
        }
        input SpotifyFilter @join__type(graph: SPOTIFY) {
          slug: String
        }
    "#;

    fn ownership() -> Ownership {
        Ownership::from(&parse_schema::<String>(SUPERGRAPH).unwrap().into_static())
    }

    fn json(value: serde_json::Value) -> Value<'static> {
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::String(s) => Value::String(s.into()),
            serde_json::Value::Array(a) => Value::Array(a.into_iter().map(json).collect()),
            serde_json::Value::Object(o) => Value::Object(
                o.into_iter()
                    .map(|(k, v)| (&*Box::leak(k.into_boxed_str()), json(v)))
                    .collect(),
            ),
            serde_json::Value::Number(n) => Value::I64(n.as_i64().unwrap()),
        }
    }

    fn names(value: &Value, key: &str) -> Vec<String> {
        match get(value, key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|i| str_of(i, "name").map(String::from))
                .collect(),
            _ => vec![],
        }
    }

    #[test]
    fn ownership_follows_resolvable_joins() {
        let ownership = ownership();
        assert_eq!(ownership.types["SpotifyAccount"], vec!["spotify"]);
        assert_eq!(ownership.types["Node"], vec!["node", "spotify"]);
        assert_eq!(ownership.fields["Query"]["maccasDeals"], vec!["maccas"]);
        assert!(!ownership.fields["SpotifyAccount"].contains_key("id"));
    }

    #[test]
    fn ownership_of_the_composed_supergraph() {
        let document = parse_schema::<String>(include_str!("../supergraph/supergraph.graphql"))
            .unwrap()
            .into_static();
        let ownership = Ownership::from(&document);
        assert_eq!(ownership.types["SpotifyAccount"], vec!["spotify"]);
        assert_eq!(ownership.fields["Query"]["homeAuth"], vec!["home"]);
        assert_eq!(ownership.fields["Query"]["node"], vec!["node"]);
    }

    #[test]
    fn introspection_hides_denied_subgraphs() {
        let ownership = ownership();
        let denied = HashSet::from(["spotify".to_owned()]);
        let hidden = Hidden {
            ownership: &ownership,
            denied: &denied,
        };

        let named =
            |name: &str| serde_json::json!({"kind": "OBJECT", "name": name, "ofType": null});
        let field = |name: &str, ty: serde_json::Value, args: serde_json::Value| serde_json::json!({"name": name, "type": ty, "args": args});
        let mut data = json(serde_json::json!({
            "__schema": {
                "queryType": {"name": "Query"},
                "mutationType": {"name": "Mutation"},
                "types": [
                    {"name": "Query", "fields": [
                        field("node", named("Node"), serde_json::json!([])),
                        field("spotifyAccount", named("SpotifyAccount"), serde_json::json!([])),
                        field("maccasDeals", serde_json::json!({"kind": "NON_NULL", "name": null, "ofType": named("String")}), serde_json::json!([])),
                    ]},
                    {"name": "Mutation", "fields": [field("spotifyLink", named("Boolean"), serde_json::json!([]))]},
                    {"name": "Node", "possibleTypes": [{"name": "SpotifyAccount"}]},
                    {"name": "SpotifyAccount", "fields": []},
                    {"name": "SpotifyFilter", "inputFields": []},
                    {"name": "String"},
                ],
            },
            "__type": {"name": "SpotifyAccount"},
        }));

        hidden.prune_response(&mut data);

        let schema = get(&data, "__schema").unwrap();
        let types = match get(schema, "types") {
            Some(Value::Array(types)) => types,
            _ => panic!("types"),
        };
        assert_eq!(
            types
                .iter()
                .filter_map(|t| str_of(t, "name"))
                .collect::<Vec<_>>(),
            vec!["Query", "Node", "String"]
        );
        assert_eq!(names(&types[0], "fields"), vec!["node", "maccasDeals"]);
        assert!(names(&types[1], "possibleTypes").is_empty());
        assert!(matches!(get(schema, "mutationType"), Some(Value::Null)));
        assert!(matches!(get(schema, "queryType"), Some(Value::Object(_))));
        assert!(matches!(get(&data, "__type"), Some(Value::Null)));
    }

    #[test]
    fn arguments_of_hidden_input_types_hide_the_field() {
        let mut ownership = ownership();
        ownership
            .fields
            .get_mut("Query")
            .unwrap()
            .remove("spotifyAccount");
        let denied = HashSet::from(["spotify".to_owned()]);
        let hidden = Hidden {
            ownership: &ownership,
            denied: &denied,
        };

        let field = json(serde_json::json!({
            "name": "spotifyAccount",
            "type": {"name": "String"},
            "args": [{"name": "filter", "type": {"name": "SpotifyFilter"}}],
        }));
        assert!(hidden.field_hidden(Some("Query"), &field));
    }

    #[test]
    fn groups_match_by_short_name_or_spn() {
        let held = vec![
            "00000000-0000-0000-0000-000000000002".to_owned(),
            "spotify_users@idm.example.test".to_owned(),
        ];
        assert!(groups_match(&held, &["spotify_users".to_owned()]));
        assert!(groups_match(
            &held,
            &["spotify_users@idm.example.test".to_owned()]
        ));
        assert!(!groups_match(&held, &["platform_admins".to_owned()]));
        assert!(!groups_match(&[], &["spotify_users".to_owned()]));
    }

    #[test]
    fn gates_need_one_of_their_issuers() {
        let gate = Gate {
            issuers: vec!["https://idm.example.test/oauth2/openid/spotify".to_owned()],
            groups: vec!["spotify_users".to_owned()],
        };
        let held = vec!["spotify_users@idm.example.test".to_owned()];
        assert!(gate.admits(
            Some("https://idm.example.test/oauth2/openid/spotify"),
            &held
        ));
        assert!(!gate.admits(
            Some("https://idm.example.test/oauth2/openid/home-gateway"),
            &held
        ));
        assert!(!gate.admits(None, &held));
    }
}
