//! Hides from introspection what only denied subgraphs serve: types and fields are owned by the
//! subgraphs their `@join__type`/`@join__field` resolve in.

use std::collections::{HashMap, HashSet};

use hive_router::{
    executor::response::value::Value,
    graphql_tools::{
        parser::schema::{Definition, Directive, TypeDefinition},
        static_graphql::schema::Document,
    },
};

#[derive(Default)]
pub struct Ownership {
    types: HashMap<String, Vec<String>>,
    fields: HashMap<String, HashMap<String, Vec<String>>>,
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

pub struct Hidden<'h> {
    pub ownership: &'h Ownership,
    pub denied: &'h HashSet<String>,
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

    pub fn prune_response(&self, data: &mut Value) {
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
        let document = parse_schema::<String>(include_str!("../../supergraph/supergraph.graphql"))
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
}
