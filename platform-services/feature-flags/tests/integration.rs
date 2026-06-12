use insta::assert_yaml_snapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use feature_flags::convert;
use feature_flags::grpc::{AdminService, EvaluationService};
use feature_flags::pb;
use feature_flags::pb::admin_client::AdminClient;
use feature_flags::pb::admin_server::AdminServer;
use feature_flags::pb::evaluation_client::EvaluationClient;
use feature_flags::pb::evaluation_server::EvaluationServer;
use feature_flags::snapshot::SnapshotManager;
use feature_flags::store::Store;

#[derive(Deserialize)]
struct Fixture {
    #[serde(default)]
    segments: Vec<SegmentDef>,
    #[serde(default)]
    flags: Vec<FlagDef>,
    #[serde(default)]
    resolve: Option<ResolveDef>,
    #[serde(default)]
    resolve_all: Option<ContextDef>,
}

#[derive(Deserialize)]
struct FlagDef {
    key: String,
    value_type: String,
    #[serde(default = "default_true")]
    enabled: bool,
    default_variant: String,
    variants: Vec<VariantDef>,
    #[serde(default)]
    rules: Vec<RuleDef>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct VariantDef {
    key: String,
    value: Value,
}

#[derive(Deserialize)]
struct RuleDef {
    #[serde(default)]
    segment: Option<String>,
    #[serde(default)]
    variant: Option<String>,
    #[serde(default)]
    distributions: Vec<DistDef>,
    #[serde(default)]
    constraint_groups: Vec<Vec<ConstraintDef>>,
    #[serde(default)]
    bucket_salt: String,
}

#[derive(Deserialize)]
struct DistDef {
    variant: String,
    weight: u32,
}

#[derive(Deserialize)]
struct ConstraintDef {
    attribute: String,
    operator: String,
    #[serde(default)]
    values: Vec<Value>,
}

#[derive(Deserialize)]
struct SegmentDef {
    key: String,
    name: String,
    #[serde(default)]
    constraints: Vec<ConstraintDef>,
}

#[derive(Deserialize)]
struct ResolveDef {
    kind: String,
    flag_key: String,
    #[serde(default)]
    context: ContextDef,
}

#[derive(Deserialize, Default)]
struct ContextDef {
    #[serde(default)]
    targeting_key: String,
    #[serde(default)]
    attributes: Value,
}

fn value_type(s: &str) -> pb::ValueType {
    match s {
        "boolean" => pb::ValueType::Boolean,
        "string" => pb::ValueType::String,
        "integer" => pb::ValueType::Integer,
        "float" => pb::ValueType::Float,
        "object" => pb::ValueType::Object,
        other => panic!("unknown value_type: {other}"),
    }
}

fn operator(s: &str) -> pb::ConstraintOperator {
    use pb::ConstraintOperator as Op;
    match s {
        "eq" => Op::Eq,
        "neq" => Op::Neq,
        "in" => Op::In,
        "not_in" => Op::NotIn,
        "contains" => Op::Contains,
        "starts_with" => Op::StartsWith,
        "ends_with" => Op::EndsWith,
        "gt" => Op::Gt,
        "gte" => Op::Gte,
        "lt" => Op::Lt,
        "lte" => Op::Lte,
        "exists" => Op::Exists,
        "regex" => Op::Regex,
        "flag_matches" => Op::FlagMatches,
        other => panic!("unknown operator: {other}"),
    }
}

fn constraint(c: &ConstraintDef) -> pb::Constraint {
    pb::Constraint {
        attribute: c.attribute.clone(),
        operator: operator(&c.operator) as i32,
        values: c.values.iter().map(convert::json_to_prost_value).collect(),
    }
}

fn eval_context(ctx: &ContextDef) -> pb::EvaluationContext {
    let attributes = ctx
        .attributes
        .is_object()
        .then(|| convert::json_to_struct(&ctx.attributes));
    pb::EvaluationContext {
        targeting_key: ctx.targeting_key.clone(),
        attributes,
    }
}

#[derive(Serialize)]
struct ResolveSnapshot {
    value: Value,
    variant: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
}

fn meta_snapshot(value: Value, meta: Option<pb::ResolutionMeta>) -> ResolveSnapshot {
    let meta = meta.unwrap_or_default();
    let reason = format!("{:?}", meta.reason());
    ResolveSnapshot {
        value,
        variant: meta.variant,
        reason,
        error_code: (!meta.error_code.is_empty()).then_some(meta.error_code),
        error_message: (!meta.error_message.is_empty()).then_some(meta.error_message),
    }
}

macro_rules! fixture_test {
    ($name:ident, $dir:literal, $file:literal) => {
        #[sqlx::test(migrations = "./migrations")]
        #[serial_test::serial]
        async fn $name(pool: PgPool) {
            run_fixture(pool, $dir, $file).await;
        }
    };
}

fixture_test!(
    resolve_targeting_match,
    "resolve",
    "boolean-targeting-match"
);
fixture_test!(resolve_default, "resolve", "boolean-default");
fixture_test!(resolve_disabled, "resolve", "disabled");
fixture_test!(resolve_flag_not_found, "resolve", "flag-not-found");
fixture_test!(resolve_type_mismatch, "resolve", "type-mismatch");
fixture_test!(
    resolve_string_inline_constraint,
    "resolve",
    "string-inline-constraint"
);
fixture_test!(resolve_cnf_and_or_match, "resolve", "cnf-and-or-match");
fixture_test!(resolve_cnf_and_or_miss, "resolve", "cnf-and-or-miss");
fixture_test!(
    resolve_segment_and_inline_match,
    "resolve",
    "segment-and-inline-match"
);
fixture_test!(
    resolve_segment_and_inline_miss,
    "resolve",
    "segment-and-inline-miss"
);
fixture_test!(
    resolve_multi_constraint_segment,
    "resolve",
    "multi-constraint-segment"
);
fixture_test!(resolve_object_default, "resolve", "object-default");
fixture_test!(resolve_prerequisite_met, "resolve", "prerequisite-met");
fixture_test!(resolve_prerequisite_unmet, "resolve", "prerequisite-unmet");
fixture_test!(resolve_all_mixed, "resolve_all", "mixed");

async fn run_fixture(pool: PgPool, dir: &str, file: &str) {
    let snapshot_name = format!("{dir}__{file}");
    let content = std::fs::read_to_string(format!("tests/fixtures/{dir}/{file}.yaml")).unwrap();
    let fixture: Fixture = serde_yaml::from_str(&content).unwrap();

    let store = Store::new(pool);
    let manager = SnapshotManager::bootstrap(store.clone(), None)
        .await
        .unwrap();

    let admin = AdminService::new(store, manager.clone());
    let evaluation = EvaluationService::new(manager);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let server_handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EvaluationServer::new(evaluation))
            .add_service(AdminServer::new(admin))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let endpoint = format!("http://{addr}");
    let mut admin_client = connect_admin(&endpoint).await;
    let mut eval_client = EvaluationClient::connect(endpoint).await.unwrap();

    seed(&mut admin_client, &fixture).await;

    let snapshot = if let Some(resolve) = &fixture.resolve {
        let result = run_resolve(&mut eval_client, resolve).await;
        SnapshotOutput::Resolve(result)
    } else if let Some(ctx) = &fixture.resolve_all {
        let result = run_resolve_all(&mut eval_client, ctx).await;
        SnapshotOutput::ResolveAll(result)
    } else {
        panic!("fixture must define `resolve` or `resolve_all`");
    };

    server_handle.abort();

    assert_yaml_snapshot!(snapshot_name, snapshot);
}

#[derive(Serialize)]
#[serde(untagged)]
enum SnapshotOutput {
    Resolve(ResolveSnapshot),
    ResolveAll(Vec<EvaluatedSnapshot>),
}

#[derive(Serialize)]
struct EvaluatedSnapshot {
    flag_key: String,
    #[serde(flatten)]
    resolution: ResolveSnapshot,
}

async fn connect_admin(endpoint: &str) -> AdminClient<tonic::transport::Channel> {
    AdminClient::connect(endpoint.to_string()).await.unwrap()
}

async fn seed(client: &mut AdminClient<tonic::transport::Channel>, fixture: &Fixture) {
    for seg in &fixture.segments {
        let segment = pb::Segment {
            key: seg.key.clone(),
            name: seg.name.clone(),
            constraints: seg.constraints.iter().map(constraint).collect(),
        };
        client
            .create_segment(pb::CreateSegmentRequest {
                segment: Some(segment),
            })
            .await
            .unwrap();
    }

    for flag in &fixture.flags {
        let variants = flag
            .variants
            .iter()
            .map(|v| pb::Variant {
                key: v.key.clone(),
                value: Some(convert::json_to_prost_value(&v.value)),
            })
            .collect();
        client
            .create_flag(pb::CreateFlagRequest {
                key: flag.key.clone(),
                value_type: value_type(&flag.value_type) as i32,
                enabled: flag.enabled,
                default_variant_key: flag.default_variant.clone(),
                variants,
            })
            .await
            .unwrap();

        if flag.rules.is_empty() {
            continue;
        }
        let rules = flag
            .rules
            .iter()
            .enumerate()
            .map(|(rank, r)| pb::Rule {
                rank: rank as u32,
                segment_key: r.segment.clone().unwrap_or_default(),
                variant_key: r.variant.clone().unwrap_or_default(),
                distributions: r
                    .distributions
                    .iter()
                    .map(|d| pb::Distribution {
                        variant_key: d.variant.clone(),
                        weight: d.weight,
                    })
                    .collect(),
                constraint_groups: r
                    .constraint_groups
                    .iter()
                    .map(|group| pb::ConstraintGroup {
                        constraints: group.iter().map(constraint).collect(),
                    })
                    .collect(),
                bucket_salt: r.bucket_salt.clone(),
            })
            .collect();
        client
            .set_flag_rules(pb::SetFlagRulesRequest {
                flag_key: flag.key.clone(),
                rules,
            })
            .await
            .unwrap();
    }
}

/// Wrap an evaluation message in a request carrying the `client-id` header the backend
/// now requires on the read path.
fn eval_request<T>(msg: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(msg);
    request
        .metadata_mut()
        .insert("client-id", "integration-test".parse().unwrap());
    request
}

async fn run_resolve(
    client: &mut EvaluationClient<tonic::transport::Channel>,
    resolve: &ResolveDef,
) -> ResolveSnapshot {
    let request = pb::ResolveRequest {
        flag_key: resolve.flag_key.clone(),
        context: Some(eval_context(&resolve.context)),
    };
    match resolve.kind.as_str() {
        "boolean" => {
            let r = client
                .resolve_boolean(eval_request(request))
                .await
                .unwrap()
                .into_inner();
            meta_snapshot(Value::from(r.value), r.meta)
        }
        "string" => {
            let r = client
                .resolve_string(eval_request(request))
                .await
                .unwrap()
                .into_inner();
            meta_snapshot(Value::from(r.value), r.meta)
        }
        "integer" => {
            let r = client
                .resolve_integer(eval_request(request))
                .await
                .unwrap()
                .into_inner();
            meta_snapshot(Value::from(r.value), r.meta)
        }
        "float" => {
            let r = client
                .resolve_float(eval_request(request))
                .await
                .unwrap()
                .into_inner();
            meta_snapshot(Value::from(r.value), r.meta)
        }
        "object" => {
            let r = client
                .resolve_object(eval_request(request))
                .await
                .unwrap()
                .into_inner();
            let value = r
                .value
                .as_ref()
                .map(|s| convert::prost_value_to_json(&prost_struct_value(s)))
                .unwrap_or(Value::Null);
            meta_snapshot(value, r.meta)
        }
        other => panic!("unknown resolve kind: {other}"),
    }
}

fn prost_struct_value(s: &prost_types::Struct) -> prost_types::Value {
    prost_types::Value {
        kind: Some(prost_types::value::Kind::StructValue(s.clone())),
    }
}

async fn run_resolve_all(
    client: &mut EvaluationClient<tonic::transport::Channel>,
    ctx: &ContextDef,
) -> Vec<EvaluatedSnapshot> {
    let request = pb::ResolveAllRequest {
        context: Some(eval_context(ctx)),
    };
    let mut flags: Vec<_> = client
        .resolve_all(eval_request(request))
        .await
        .unwrap()
        .into_inner()
        .flags
        .into_iter()
        .map(|f| {
            let value = f
                .value
                .as_ref()
                .map(convert::prost_value_to_json)
                .unwrap_or(Value::Null);
            EvaluatedSnapshot {
                flag_key: f.flag_key.clone(),
                resolution: meta_snapshot(value, f.meta),
            }
        })
        .collect();
    flags.sort_by(|a, b| a.flag_key.cmp(&b.flag_key));
    flags
}

#[sqlx::test(migrations = "./migrations")]
async fn list_changes_records_and_filters_audit_log(pool: PgPool) {
    let store = Store::new(pool);
    let variants = [feature_flags::model::Variant {
        key: "on".into(),
        value: serde_json::Value::Bool(true),
    }];
    store
        .create_flag(
            "alice",
            "flag-a",
            feature_flags::model::ValueType::Boolean,
            true,
            "on",
            &variants,
        )
        .await
        .unwrap();
    store
        .update_flag("bob", "flag-a", false, "on")
        .await
        .unwrap();

    let all = store.list_changes("", "", 0).await.unwrap();
    assert_eq!(all.len(), 2);
    // Newest first: the update precedes the create.
    assert_eq!(all[0].action, "update_flag");
    assert_eq!(all[0].actor, "bob");
    assert_eq!(all[1].action, "create_flag");
    assert_eq!(all[1].actor, "alice");

    // Filtering by target narrows to that flag's rows.
    let filtered = store.list_changes("flag", "flag-a", 0).await.unwrap();
    assert_eq!(filtered.len(), 2);
    let other = store.list_changes("segment", "nope", 0).await.unwrap();
    assert!(other.is_empty());

    // The limit caps the returned rows.
    let limited = store.list_changes("", "", 1).await.unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].action, "update_flag");
}

#[sqlx::test(migrations = "./migrations")]
async fn resolve_requires_client_id(pool: PgPool) {
    let store = Store::new(pool);
    let manager = SnapshotManager::bootstrap(store.clone(), None)
        .await
        .unwrap();
    let evaluation = EvaluationService::new(manager);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let server_handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EvaluationServer::new(evaluation))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let mut client = EvaluationClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let request = pb::ResolveRequest {
        flag_key: "anything".into(),
        context: None,
    };

    // Without the `client-id` header the request is rejected.
    let err = client
        .resolve_boolean(request.clone())
        .await
        .expect_err("expected rejection");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // With it, the request is served (flag missing, but the call itself succeeds).
    let ok = client.resolve_boolean(eval_request(request)).await;
    assert!(ok.is_ok());

    server_handle.abort();
}
