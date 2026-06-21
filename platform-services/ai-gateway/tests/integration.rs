use std::collections::HashMap;

use insta::assert_yaml_snapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use ai_gateway::config::{Config, ProviderConfig, Rule};
use ai_gateway::feature_flag::FeatureFlagClient;
use ai_gateway::pricing::Pricing;
use ai_gateway::providers::{Dialect, Registry};
use ai_gateway::server;
use ai_gateway::state::AppState;

const API_KEY_ENV: &str = "AIG_TEST_UPSTREAM_KEY";

#[derive(Deserialize)]
struct Fixture {
    endpoint: String,
    provider: ProviderDef,
    #[serde(default)]
    key: KeyDef,
    request: Value,
    upstream: Option<Upstream>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct ProviderDef {
    #[serde(default = "default_provider_name")]
    name: String,
    dialect: Dialect,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    embedding_models: Vec<String>,
}

fn default_provider_name() -> String {
    "test".into()
}

#[derive(Deserialize)]
#[serde(default)]
struct KeyDef {
    allowed_models: Vec<String>,
    monthly_token_budget: Option<i64>,
    auth: String,
}

impl Default for KeyDef {
    fn default() -> Self {
        Self {
            allowed_models: Vec::new(),
            monthly_token_budget: None,
            auth: "valid".into(),
        }
    }
}

#[derive(Deserialize)]
struct Upstream {
    #[serde(default = "default_status")]
    status: u16,
    /// JSON body for a buffered response.
    #[serde(default)]
    body: Option<Value>,
    /// Raw `text/event-stream` body for a streamed response.
    #[serde(default)]
    sse: Option<String>,
}

fn default_status() -> u16 {
    200
}

#[derive(Serialize)]
struct Snapshot {
    response: ResponseSnapshot,
    upstream_requests: Vec<CapturedRequest>,
}

#[derive(Serialize)]
struct ResponseSnapshot {
    status: u16,
    body: Value,
}

#[derive(Serialize)]
struct CapturedRequest {
    method: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
}

fn capture_requests(requests: &[Request]) -> Vec<CapturedRequest> {
    requests
        .iter()
        .map(|r| CapturedRequest {
            method: r.method.to_string(),
            path: r.url.path().to_string(),
            body: if r.body.is_empty() {
                None
            } else {
                serde_json::from_slice(&r.body).ok()
            },
        })
        .collect()
}

/// `#[sqlx::test]` hands each test an isolated, freshly-migrated database. Fixtures are
/// declared explicitly via `fixture_test!` and serialized, since the gateway's feature
/// flag client and the upstream API-key env var are process-global.
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
    messages_anthropic_happy_path,
    "messages",
    "anthropic-happy-path"
);
fixture_test!(messages_budget_exceeded, "messages", "budget-exceeded");
fixture_test!(messages_invalid_key, "messages", "invalid-key");
fixture_test!(messages_missing_key, "messages", "missing-key");
fixture_test!(messages_model_not_allowed, "messages", "model-not-allowed");
fixture_test!(
    messages_endpoint_to_openai_provider,
    "messages",
    "endpoint-to-openai-provider"
);
fixture_test!(messages_model_override, "messages", "model-override");
fixture_test!(messages_model_denied, "messages", "model-denied");
fixture_test!(messages_route_pinned, "messages", "route-pinned");
fixture_test!(
    messages_anthropic_streaming,
    "messages",
    "anthropic-streaming"
);
fixture_test!(
    messages_streaming_openai_provider,
    "messages",
    "streaming-openai-provider"
);
fixture_test!(chat_openai_streaming, "chat", "openai-streaming");
fixture_test!(
    chat_streaming_anthropic_provider,
    "chat",
    "streaming-anthropic-provider"
);
fixture_test!(chat_openai_happy_path, "chat", "openai-happy-path");
fixture_test!(chat_no_provider_for_model, "chat", "no-provider-for-model");
fixture_test!(
    chat_endpoint_to_anthropic_provider,
    "chat",
    "endpoint-to-anthropic-provider"
);
fixture_test!(
    embeddings_openai_happy_path,
    "embeddings",
    "openai-happy-path"
);
fixture_test!(
    embeddings_no_provider_for_model,
    "embeddings",
    "no-provider-for-model"
);

async fn run_fixture(pool: PgPool, dir: &str, file: &str) {
    let snapshot_name = format!("{dir}__{file}");
    let content = std::fs::read_to_string(format!("tests/fixtures/{dir}/{file}.yaml")).unwrap();
    let fixture: Fixture = serde_yaml::from_str(&content).unwrap();

    let upstream = MockServer::start().await;
    if let Some(up) = &fixture.upstream {
        let template = ResponseTemplate::new(up.status);
        let template = match (&up.sse, &up.body) {
            (Some(sse), _) => template
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse.clone()),
            (None, Some(body)) => template.set_body_json(body),
            (None, None) => template,
        };
        Mock::given(method("POST"))
            .respond_with(template)
            .mount(&upstream)
            .await;
    }
    // SAFETY: tests are serialized, so the shared process env is not raced.
    unsafe { std::env::set_var(API_KEY_ENV, "secret") };

    let mut providers = HashMap::new();
    providers.insert(
        fixture.provider.name.clone(),
        ProviderConfig {
            dialect: fixture.provider.dialect,
            base_url: upstream.uri(),
            api_key_env: Some(API_KEY_ENV.into()),
            models: fixture.provider.models.clone(),
            embedding_models: fixture.provider.embedding_models.clone(),
            priority: 100,
        },
    );
    let config = Config {
        admin_token: String::new(),
        providers,
        keys: vec![],
        rules: fixture.rules.clone(),
    };
    let registry = Registry::from_config(&config);

    // No feature-flags backend in tests: the NoOp provider returns every flag's default
    // (enabled, no model override), matching the production-on configuration. Pointing the
    // client at a bogus URL would instead block ~120s per test on a connect timeout before
    // falling back to the same defaults.
    let features = FeatureFlagClient::new(None).await;
    let state = AppState::new(config, registry, pool, features, Pricing::default(), None);

    let token = match fixture.key.auth.as_str() {
        "valid" => {
            let (raw, _) = state
                .keys
                .create(
                    &format!("it-{snapshot_name}"),
                    &fixture.key.allowed_models,
                    fixture.key.monthly_token_budget,
                )
                .await
                .unwrap();
            Some(raw)
        }
        "invalid" => Some("aig_definitelynotarealkey".to_string()),
        "missing" => None,
        other => panic!("unknown auth mode: {other}"),
    };

    let app = server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}{}", fixture.endpoint))
        .json(&fixture.request);
    if let Some(token) = &token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.expect("request failed");

    let streaming = fixture
        .request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let status = resp.status().as_u16();
    let body = if streaming {
        // The forwarded SSE is snapshotted as the ordered list of its `data:` payloads.
        sse_events(&resp.text().await.unwrap_or_default())
    } else {
        resp.json().await.unwrap_or(Value::Null)
    };

    // Successful responses must conform to the client dialect's published schema, so a
    // translation change that breaks API compatibility fails even if its snapshot is accepted.
    if status == 200 {
        validate_schema(&fixture.endpoint, streaming, &body);
    }

    server_handle.abort();

    let upstream_requests =
        capture_requests(&upstream.received_requests().await.unwrap_or_default());

    let snapshot = Snapshot {
        response: ResponseSnapshot { status, body },
        upstream_requests,
    };

    // Translated streams synthesize ids and timestamps; redact them so snapshots stay stable.
    assert_yaml_snapshot!(snapshot_name, snapshot, {
        ".response.body.created" => "[created]",
        ".response.body[].id" => "[id]",
        ".response.body[].created" => "[created]",
        ".response.body[].message.id" => "[id]",
    });
}

/// Asserts the response conforms to the client dialect's published JSON Schema. Buffered
/// responses are validated whole; streamed responses validate each SSE event. Runs on the
/// real body, before snapshot redaction, so schema breaks surface independently of snapshots.
fn validate_schema(endpoint: &str, streaming: bool, body: &Value) {
    let schema_src = if endpoint.ends_with("/messages") {
        if streaming {
            include_str!("schemas/anthropic-stream-event.json")
        } else {
            include_str!("schemas/anthropic-message.json")
        }
    } else if endpoint.ends_with("/embeddings") {
        include_str!("schemas/openai-embeddings.json")
    } else if streaming {
        include_str!("schemas/openai-chat-chunk.json")
    } else {
        include_str!("schemas/openai-chat.json")
    };

    let schema: Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).expect("invalid test schema");

    let instances: Vec<&Value> = match body {
        Value::Array(events) if streaming => events.iter().collect(),
        other => vec![other],
    };
    for instance in instances {
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("  {} at {}", e, e.instance_path))
            .collect();
        assert!(
            errors.is_empty(),
            "{endpoint} response violates schema:\n{}\ninstance: {instance}",
            errors.join("\n"),
        );
    }
}

/// Parses an SSE body into the ordered list of its `data:` JSON payloads, dropping the
/// terminal `[DONE]` marker and any `event:` lines.
fn sse_events(body: &str) -> Value {
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty() && *data != "[DONE]")
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect();
    Value::Array(events)
}
