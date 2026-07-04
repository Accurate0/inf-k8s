use std::collections::HashMap;
use std::path::PathBuf;
use std::slice;
use std::sync::Arc;

use insta::assert_yaml_snapshot;
use insta::assert_snapshot;
use insta::internals::Content;
use janitor_bot::feature_flag::FeatureFlagClient;
use janitor_bot::llm::LlmClient;
use rstest::rstest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use janitor_bot::argocd::ArgocdClient;
use janitor_bot::clients::Clients;
use janitor_bot::dashboard;
use janitor_bot::forgejo::ForgejoClient;
use janitor_bot::github::GitHubClient;
use janitor_bot::registry::RegistryClient;
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
    #[serde(default)]
    query: HashMap<String, String>,
    status: Option<u16>,
    body: Option<Value>,
    body_text: Option<String>,
    #[serde(default)]
    service: String,
    // Stop matching after N calls, letting a lower-priority mock for the same
    // path take over — used to simulate transient responses (e.g. a 405 that
    // clears on retry).
    max_match_count: Option<u64>,
    // Lower number = higher precedence when multiple mocks match a request.
    priority: Option<u8>,
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

        let mut mock = Mock::given(method(mock_def.method.as_str())).and(path(&mock_def.path));
        for (key, value) in &mock_def.query {
            mock = mock.and(query_param(key.as_str(), value.as_str()));
        }
        let mut mock = mock.respond_with(response).expect(1..);
        if let Some(n) = mock_def.max_match_count {
            mock = mock.up_to_n_times(n);
        }
        if let Some(p) = mock_def.priority {
            mock = mock.with_priority(p);
        }
        mock.mount(server).await;
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
            // argocd gRPC paths (`/<package>.<Service>/<Method>`) vary across
            // argocd versions, so only the fact that the service was called
            // matters. The REST webhook (`/api/...`) is stable, so keep it.
            let path_with_query = if service == "argocd" && !path_with_query.starts_with("/api/") {
                "[redacted]".to_string()
            } else {
                path_with_query
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
#[serial_test::serial]
async fn evaluate_fixture(#[files("tests/fixtures/**/*.yaml")] fixture_path: PathBuf) {
    let content = std::fs::read_to_string(&fixture_path).unwrap();
    let mut fixture: Fixture = yaml_serde::from_str(&content).unwrap();

    let forgejo_server = MockServer::start().await;
    let github_server = MockServer::start().await;
    let argocd_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let registry_server = MockServer::start().await;

    let payload_str = serde_json::to_string(&fixture.payload)
        .unwrap()
        .replace("MOCK_GITHUB_URL", &github_server.uri());
    fixture.payload = serde_json::from_str(&payload_str).unwrap();

    for mock_def in &fixture.mocks {
        match mock_def.service.as_str() {
            "github" => setup_mocks(&github_server, slice::from_ref(mock_def)).await,
            "argocd" => setup_mocks(&argocd_server, slice::from_ref(mock_def)).await,
            "forgejo" => setup_mocks(&forgejo_server, slice::from_ref(mock_def)).await,
            "llm" => setup_mocks(&llm_server, slice::from_ref(mock_def)).await,
            "registry" => setup_mocks(&registry_server, slice::from_ref(mock_def)).await,
            _ => unreachable!(),
        }
    }

    let state = Arc::new(AppState {
        clients: Clients::new(
            ForgejoClient::new(forgejo_server.uri(), "test-token".into()).unwrap(),
            GitHubClient::new(github_server.uri(), "test-token".into()),
            ArgocdClient::new(argocd_server.uri(), "test-token".into()),
            FeatureFlagClient::new(None).await,
            RegistryClient::new(registry_server.uri(), None),
            Some(LlmClient::new(llm_server.uri(), "test-token".into())),
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

    forgejo_server.verify().await;
    github_server.verify().await;
    argocd_server.verify().await;
    registry_server.verify().await;

    let forgejo_requests = forgejo_server.received_requests().await.unwrap_or_default();
    let github_requests = github_server.received_requests().await.unwrap_or_default();
    let argocd_requests = argocd_server.received_requests().await.unwrap_or_default();

    let mut external_requests = capture_requests(&forgejo_requests, "forgejo");
    external_requests.extend(capture_requests(&github_requests, "github"));
    external_requests.extend(capture_requests(&argocd_requests, "argocd"));

    let snapshot = Snapshot {
        response,
        external_requests,
    };

    let parent_dir = fixture_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_stem = fixture_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let snapshot_name = format!("{parent_dir}__{file_stem}");

    let argocd_host = argocd_server
        .uri()
        .strip_prefix("http://")
        .unwrap_or(&argocd_server.uri())
        .to_string();

    let mut settings = insta::Settings::clone_current();
    settings.add_dynamic_redaction(".external_requests[].body.body", move |value, _path| {
        if let Some(s) = value.as_str() {
            let replaced = s.replace(&argocd_host, "ARGOCD_SERVER");
            // Redact argocd error messages which vary depending on environment
            let replaced = regex::Regex::new(r"(?s)```diff\nError running argocd diff.*?```")
                .unwrap()
                .replace_all(&replaced, "```diff\n[argocd diff error redacted]\n```")
                .into_owned();
            Content::from(replaced)
        } else {
            value.clone()
        }
    });
    settings.bind(|| {
        assert_yaml_snapshot!(snapshot_name, snapshot);
    });
}

fn renovate_pr(number: i64, author: &str, sha: &str, html_url: &str) -> Value {
    json!({
        "number": number,
        "title": format!("Update dependency to v{number}"),
        "user": {
            "login": author,
            "avatar_url": "",
            "html_url": "",
            "created": "2020-01-01T00:00:00Z",
            "last_login": "2020-01-01T00:00:00Z",
        },
        "labels": [
            {"id": 1, "name": "renovate", "color": "#1a7f37", "url": ""},
            {"id": 2, "name": "renovate/patch", "color": "#8957e5", "url": ""},
        ],
        "base": {"ref": "main"},
        "head": {"ref": "renovate/dep", "sha": sha},
        "mergeable": true,
        "state": "open",
        "merged": false,
        "diff_url": "",
        "html_url": html_url,
        "patch_url": "",
        "url": "",
        "requested_reviewers": [],
        "closed_at": null,
        "created_at": "2020-01-01T00:00:00Z",
        "due_date": null,
        "merged_at": null,
        "updated_at": "2020-01-01T00:00:00Z",
    })
}

#[tokio::test]
#[serial_test::serial]
async fn renovate_dashboard_snapshot() {
    let forgejo_server = MockServer::start().await;
    let unused_server = MockServer::start().await;

    let sha = "def0000000000000000000000000000000000000";

    // anurag/k8s: one renovate PR with a combined status (mixed checks).
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/anurag/k8s/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([renovate_pr(
            42,
            "renovate",
            sha,
            "https://forgejo.example/anurag/k8s/pulls/42"
        )])))
        .mount(&forgejo_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/anurag/k8s/commits/{sha}/status"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "state": "success",
            "total_count": 2,
            "statuses": [
                {
                    "context": "ci/build",
                    "status": "success",
                    "description": "build passed",
                    "target_url": "https://forgejo.example/anurag/k8s/actions/runs/1",
                },
                {
                    "context": "ci/lint",
                    "status": "pending",
                    "description": "",
                    "target_url": "",
                },
            ],
        })))
        .mount(&forgejo_server)
        .await;

    // anurag/home-gateway: no open PRs.
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/anurag/home-gateway/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&forgejo_server)
        .await;

    // anurag/solar-panels: an open PR authored by someone else, filtered out.
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/anurag/solar-panels/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([renovate_pr(
            7,
            "anurag",
            "aaa0000000000000000000000000000000000000",
            "https://forgejo.example/anurag/solar-panels/pulls/7"
        )])))
        .mount(&forgejo_server)
        .await;

    let clients = Clients::new(
        ForgejoClient::new(forgejo_server.uri(), "test-token".into()).unwrap(),
        GitHubClient::new(unused_server.uri(), "test-token".into()),
        ArgocdClient::new(unused_server.uri(), "test-token".into()),
        FeatureFlagClient::new(None).await,
        RegistryClient::new(unused_server.uri(), None),
        Some(LlmClient::new(unused_server.uri(), "test-token".into())),
    );
    let orchestrator = rules::RulesOrchestrator::new();

    let html = dashboard::render_dashboard(&clients, &orchestrator).await;
    let html = regex::Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} UTC")
        .unwrap()
        .replace_all(&html, "[generated-at]");

    assert_snapshot!("renovate_dashboard", html);
}
