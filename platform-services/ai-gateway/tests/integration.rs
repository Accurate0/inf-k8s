use std::collections::HashMap;

use insta::assert_yaml_snapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use ai_gateway::config::{Config, ProviderConfig};
use ai_gateway::feature_flag::FeatureFlagClient;
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
    body: Value,
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

fixture_test!(messages_anthropic_happy_path, "messages", "anthropic-happy-path");
fixture_test!(messages_budget_exceeded, "messages", "budget-exceeded");
fixture_test!(messages_invalid_key, "messages", "invalid-key");
fixture_test!(messages_missing_key, "messages", "missing-key");
fixture_test!(messages_model_not_allowed, "messages", "model-not-allowed");
fixture_test!(messages_endpoint_to_openai_provider, "messages", "endpoint-to-openai-provider");
fixture_test!(chat_openai_happy_path, "chat", "openai-happy-path");
fixture_test!(chat_no_provider_for_model, "chat", "no-provider-for-model");
fixture_test!(chat_endpoint_to_anthropic_provider, "chat", "endpoint-to-anthropic-provider");
fixture_test!(embeddings_openai_happy_path, "embeddings", "openai-happy-path");
fixture_test!(embeddings_no_provider_for_model, "embeddings", "no-provider-for-model");

async fn run_fixture(pool: PgPool, dir: &str, file: &str) {
    let snapshot_name = format!("{dir}__{file}");
    let content = std::fs::read_to_string(format!("tests/fixtures/{dir}/{file}.yaml")).unwrap();
    let fixture: Fixture = serde_yaml::from_str(&content).unwrap();

    let upstream = MockServer::start().await;
    if let Some(up) = &fixture.upstream {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(up.status).set_body_json(&up.body))
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
    };
    let registry = Registry::from_config(&config);

    // No feature-flags backend in tests: the NoOp provider returns every flag's default
    // (enabled, no model override), matching the production-on configuration. Pointing the
    // client at a bogus URL would instead block ~120s per test on a connect timeout before
    // falling back to the same defaults.
    let features = FeatureFlagClient::new(None).await;
    let state = AppState::new(config, registry, pool, features, None);

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

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);

    server_handle.abort();

    let upstream_requests =
        capture_requests(&upstream.received_requests().await.unwrap_or_default());

    let snapshot = Snapshot {
        response: ResponseSnapshot { status, body },
        upstream_requests,
    };

    assert_yaml_snapshot!(snapshot_name, snapshot, {
        ".response.body.created" => "[created]"
    });
}
