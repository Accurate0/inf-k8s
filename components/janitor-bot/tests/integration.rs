use std::path::PathBuf;
use std::slice;
use std::sync::Arc;

use insta::assert_yaml_snapshot;
use rstest::rstest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};
use yaml_serde;

use janitor_bot::argocd::ArgocdClient;
use janitor_bot::clients::Clients;
use janitor_bot::forgejo::ForgejoClient;
use janitor_bot::github::GitHubClient;
use janitor_bot::rules;
use janitor_bot::server::{self, AppState};

#[derive(Deserialize)]
struct Fixture {
    r#type: String,
    payload: Value,
    #[serde(default)]
    mocks: Vec<MockDef>,
    now: Option<String>,
}

#[derive(Deserialize)]
struct MockDef {
    method: String,
    path: String,
    status: Option<u16>,
    body: Option<Value>,
    body_text: Option<String>,
    #[serde(default)]
    service: String,
}

#[derive(Serialize)]
struct Snapshot {
    response: Value,
    external_requests: Vec<CapturedRequest>,
}

#[derive(Serialize)]
struct CapturedRequest {
    method: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    service: String,
}

async fn setup_mocks(server: &MockServer, mocks: &[MockDef]) {
    for mock_def in mocks {
        let status = mock_def.status.unwrap_or(200);
        let response = if let Some(body_text) = &mock_def.body_text {
            ResponseTemplate::new(status).set_body_string(body_text)
        } else if let Some(body) = &mock_def.body {
            ResponseTemplate::new(status).set_body_json(body)
        } else {
            ResponseTemplate::new(status)
        };

        Mock::given(method(mock_def.method.as_str()))
            .and(path(&mock_def.path))
            .respond_with(response)
            .mount(server)
            .await;
    }
}

fn capture_requests(requests: &[Request], service: &str) -> Vec<CapturedRequest> {
    requests
        .iter()
        .map(|r| {
            let body = if r.body.is_empty() {
                None
            } else {
                serde_json::from_slice(&r.body).ok()
            };
            let path = r.url.path().to_string();
            let query = r.url.query().unwrap_or("");
            let path_with_query = if query.is_empty() {
                path
            } else {
                format!("{path}?{query}")
            };
            CapturedRequest {
                method: r.method.to_string(),
                path: path_with_query,
                body,
                service: service.to_string(),
            }
        })
        .collect()
}

#[rstest]
#[tokio::test]
async fn evaluate_fixture(#[files("tests/fixtures/**/*.yaml")] fixture_path: PathBuf) {
    let content = std::fs::read_to_string(&fixture_path).unwrap();
    let mut fixture: Fixture = yaml_serde::from_str(&content).unwrap();

    let forgejo_server = MockServer::start().await;
    let github_server = MockServer::start().await;

    let payload_str = serde_json::to_string(&fixture.payload)
        .unwrap()
        .replace("MOCK_GITHUB_URL", &github_server.uri());
    fixture.payload = serde_json::from_str(&payload_str).unwrap();

    for mock_def in &fixture.mocks {
        if mock_def.service == "github" {
            setup_mocks(&github_server, slice::from_ref(mock_def)).await;
        } else {
            setup_mocks(&forgejo_server, slice::from_ref(mock_def)).await;
        }
    }

    let state = Arc::new(AppState {
        clients: Clients::new(
            ForgejoClient::new(forgejo_server.uri(), "test-token".into()).unwrap(),
            GitHubClient::new("test-token".into(), github_server.uri()),
            ArgocdClient::new("http://localhost".into(), "test-token".into()),
        ),
        orchestrator: rules::RulesOrchestrator::new(),
    });

    let app = server::test_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/evaluate"))
        .json(&json!({
            "type": fixture.r#type,
            "payload": fixture.payload,
            "now": fixture.now,
        }))
        .send()
        .await
        .expect("request failed");

    let response: Value = resp.json().await.expect("failed to parse response");

    server_handle.abort();

    let forgejo_requests = forgejo_server.received_requests().await.unwrap_or_default();
    let github_requests = github_server.received_requests().await.unwrap_or_default();

    let mut external_requests = capture_requests(&forgejo_requests, "forgejo");
    external_requests.extend(capture_requests(&github_requests, "github"));

    let snapshot = Snapshot {
        response,
        external_requests,
    };

    let snapshot_name = fixture_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert_yaml_snapshot!(snapshot_name, snapshot);
}
