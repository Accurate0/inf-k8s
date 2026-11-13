use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use std::sync::{Arc, Mutex};

use registry::{
    config::{Config, ForgejoConfig},
    harbor::Harbor,
    server,
    state::{AppState, Signer},
    storage::Storage,
};
use registry_api::{
    Artifact, Target,
    signing::{self, Statement},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const KANIDM_AUDIENCE: &str = "registry";
const FORGEJO_AUDIENCE: &str = "registry.anurag.sh";

struct Idp {
    issuer: String,
    kid: String,
    key: EncodingKey,
    handle: JoinHandle<()>,
}

impl Idp {
    async fn spawn(seed: u8) -> Self {
        let secret = p256::SecretKey::from_slice(&[seed; 32]).unwrap();
        let key = EncodingKey::from_ec_der(secret.to_pkcs8_der().unwrap().as_bytes());
        let kid = format!("key-{seed}");

        let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(kid);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");
        let jwks = json!({ "keys": [jwk] }).to_string();
        let userinfo = json!({ "groups": [] }).to_string();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0u8; 8192];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let body = if head.starts_with("GET /userinfo") {
                    &userinfo
                } else {
                    &jwks
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        Self {
            issuer,
            kid,
            key,
            handle,
        }
    }

    fn sign(&self, mut claims: Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        claims["iss"] = json!(self.issuer);
        claims["exp"] = json!(chrono::Utc::now().timestamp() + 300);
        encode(&header, &claims, &self.key).unwrap()
    }

    fn user(&self, name: &str, groups: &[&str]) -> String {
        self.sign(json!({
            "aud": KANIDM_AUDIENCE,
            "sub": format!("sub-{name}"),
            "preferred_username": name,
            "groups": groups,
        }))
    }

    fn forgejo(&self, overrides: Value) -> String {
        let mut claims = json!({
            "aud": FORGEJO_AUDIENCE,
            "sub": "repo:anurag/tools:ref:refs/heads/main",
            "repository": "anurag/tools",
            "repository_id": "42",
            "ref": "refs/heads/main",
            "sha": "deadbeef",
            "run_id": "7",
            "event_name": "push",
            "workflow_ref": "anurag/tools/.forgejo/workflows/publish.yaml@refs/heads/main",
        });
        for (k, v) in overrides.as_object().unwrap() {
            claims[k] = v.clone();
        }
        self.sign(claims)
    }
}

impl Drop for Idp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

type Manifests = std::collections::HashMap<String, (String, Vec<u8>)>;

#[derive(Clone, Default)]
struct HarborCalls {
    created: Arc<Mutex<Vec<Value>>>,
    deleted: Arc<Mutex<Vec<i64>>>,
    blobs: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    manifests: Arc<Mutex<Manifests>>,
    uploads: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl HarborCalls {
    fn manifest(&self, repository: &str, reference: &str) -> Option<Value> {
        let manifests = self.manifests.lock().unwrap();
        let (_, body) = manifests.get(&format!("{repository}@{reference}"))?;
        Some(serde_json::from_slice(body).unwrap())
    }

    fn blob(&self, digest: &str) -> Option<Vec<u8>> {
        self.blobs.lock().unwrap().get(digest).cloned()
    }
}

fn oci_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", sha(bytes))
}

async fn mock_oci(
    axum::extract::State(calls): axum::extract::State<HarborCalls>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::{
        http::{Method, StatusCode, header},
        response::IntoResponse,
    };

    let path = uri.path().trim_start_matches("/v2/").to_owned();
    let query = uri.query().unwrap_or_default().to_owned();

    if path.is_empty() {
        return StatusCode::OK.into_response();
    }

    if let Some((repository, rest)) = path.split_once("/blobs/uploads/") {
        let id = if rest.is_empty() {
            let id = format!("u{}", calls.uploads.lock().unwrap().len());
            calls.uploads.lock().unwrap().insert(id.clone(), Vec::new());
            id
        } else {
            rest.to_owned()
        };
        if method == Method::POST {
            return (
                StatusCode::ACCEPTED,
                [(
                    header::LOCATION,
                    format!(
                        "https://registry.example.test/v2/{repository}/blobs/uploads/{id}?_state=x"
                    ),
                )],
            )
                .into_response();
        }
        let mut uploads = calls.uploads.lock().unwrap();
        let Some(data) = uploads.get_mut(&id) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        data.extend_from_slice(&body);
        if method == Method::PATCH {
            return (
                StatusCode::ACCEPTED,
                [(
                    header::LOCATION,
                    format!("/v2/{repository}/blobs/uploads/{id}?_state=y"),
                )],
            )
                .into_response();
        }
        let digest = query
            .split('&')
            .find_map(|p| p.strip_prefix("digest="))
            .unwrap()
            .replace("%3A", ":");
        let data = uploads.remove(&id).unwrap();
        if oci_digest(&data) != digest {
            return StatusCode::BAD_REQUEST.into_response();
        }
        calls.blobs.lock().unwrap().insert(digest, data);
        return StatusCode::CREATED.into_response();
    }

    if let Some((_, digest)) = path.split_once("/blobs/") {
        let Some(blob) = calls.blob(digest) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        return if method == Method::HEAD {
            [(header::CONTENT_LENGTH, blob.len().to_string())].into_response()
        } else {
            blob.into_response()
        };
    }

    if let Some((repository, reference)) = path.split_once("/manifests/") {
        let key = format!("{repository}@{reference}");
        if method == Method::PUT {
            let media_type = headers
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let mut manifests = calls.manifests.lock().unwrap();
            if !reference.starts_with("sha256:")
                && reference.contains('.')
                && manifests.contains_key(&key)
            {
                return StatusCode::PRECONDITION_FAILED.into_response();
            }
            let entry = (media_type, body.to_vec());
            manifests.insert(format!("{repository}@{}", oci_digest(&body)), entry.clone());
            manifests.insert(key, entry);
            return StatusCode::CREATED.into_response();
        }
        let Some((media_type, body)) = calls.manifests.lock().unwrap().get(&key).cloned() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        return ([(header::CONTENT_TYPE, media_type)], body).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

struct MockHarbor {
    url: String,
    calls: HarborCalls,
    handle: JoinHandle<()>,
}

impl MockHarbor {
    async fn spawn() -> Self {
        use axum::{
            Json, Router,
            extract::{Path, State},
            http::StatusCode,
            routing::{delete, post},
        };

        async fn create(
            State(calls): State<HarborCalls>,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let mut created = calls.created.lock().unwrap();
            created.push(body.clone());
            let id = created.len() as i64;
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": id,
                    "name": format!("robot${}", body["name"].as_str().unwrap()),
                    "secret": format!("secret-{id}"),
                    "expires_at": chrono::Utc::now().timestamp() + 86400,
                })),
            )
        }

        async fn remove(State(calls): State<HarborCalls>, Path(id): Path<i64>) -> StatusCode {
            calls.deleted.lock().unwrap().push(id);
            StatusCode::OK
        }

        let calls = HarborCalls::default();
        let app = Router::new()
            .route("/api/v2.0/robots", post(create))
            .route("/api/v2.0/robots/{id}", delete(remove))
            .route("/v2/", axum::routing::any(mock_oci))
            .route("/v2/{*path}", axum::routing::any(mock_oci))
            .with_state(calls.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self { url, calls, handle }
    }
}

impl Drop for MockHarbor {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct Registry {
    url: String,
    http: reqwest::Client,
    handle: JoinHandle<()>,
    kanidm: Idp,
    forgejo: Idp,
    harbor: MockHarbor,
    signer: Signer,
    admin: String,
}

impl Registry {
    async fn spawn(pool: PgPool) -> Self {
        let kanidm = Idp::spawn(7).await;
        let forgejo = Idp::spawn(9).await;

        let mut config = Config::baked_in().unwrap();
        config.oauth.issuer = kanidm.issuer.clone();
        config.forgejo = Some(ForgejoConfig {
            issuer: forgejo.issuer.clone(),
            audiences: vec![FORGEJO_AUDIENCE.to_owned()],
            jwks_uri: None,
        });
        config.limits.max_artifact_bytes = 1024;

        let harbor = MockHarbor::spawn().await;
        let signer = Signer::new(signing::generate());
        let mut state = AppState::new(config, pool, Storage::memory(), signer.clone());
        state.harbor = Some(Harbor::new(
            &harbor.url,
            "admin".to_owned(),
            "password".to_owned(),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = server::router(state);
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let admin = kanidm.user("anurag", &["registry_admins@idm.example.test"]);

        Self {
            url,
            http: reqwest::Client::new(),
            handle,
            kanidm,
            forgejo,
            harbor,
            signer,
            admin,
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .http
            .request(method.parse().unwrap(), format!("{}{path}", self.url));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn upload(&self, token: &str, path: &str, bytes: &[u8], sha: &str) -> (u16, Value) {
        let response = self
            .http
            .put(format!("{}{path}", self.url))
            .bearer_auth(token)
            .header("x-ortzi-sha256", sha)
            .body(bytes.to_vec())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn download(&self, token: &str, path: &str) -> (u16, Vec<u8>, Option<String>) {
        let response = self
            .http
            .get(format!("{}{path}", self.url))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let sha = response
            .headers()
            .get("x-ortzi-sha256")
            .map(|v| v.to_str().unwrap().to_owned());
        (status, response.bytes().await.unwrap().to_vec(), sha)
    }

    async fn create_package(&self, name: &str) {
        let (status, body) = self
            .call(
                "POST",
                "/cli/v1/packages",
                Some(&self.admin),
                Some(json!({ "name": name, "description": "a tool", "aliases": [format!("{name}ctl")] })),
            )
            .await;
        assert_eq!(status, 201, "{body}");
    }

    async fn publish(&self, token: &str, name: &str, version: &str) -> u16 {
        self.publish_described(token, name, version, None).await
    }

    async fn publish_described(
        &self,
        token: &str,
        name: &str,
        version: &str,
        description: Option<&str>,
    ) -> u16 {
        let base = format!("/cli/v1/packages/{name}/versions");
        let (status, body) = self
            .call(
                "POST",
                &base,
                Some(token),
                Some(json!({
                    "version": version,
                    "targets": ["darwin-arm64", "linux-amd64"],
                    "description": description,
                    "auth": {
                        "issuer": "https://idm.example.test/oauth2/openid/ff",
                        "client_id": "ff",
                        "token_env": "FFCTL_TOKEN",
                    },
                })),
            )
            .await;
        if status >= 300 {
            return status;
        }
        assert_eq!(body["status"], "draft");

        for target in ["darwin-arm64", "linux-amd64"] {
            let bytes = format!("{name}-{version}-{target}");
            let (status, body) = self
                .upload(
                    token,
                    &format!("{base}/{version}/artifacts/{target}"),
                    bytes.as_bytes(),
                    &sha(bytes.as_bytes()),
                )
                .await;
            assert_eq!(status, 200, "{body}");
        }

        let artifacts = ["darwin-arm64", "linux-amd64"]
            .into_iter()
            .map(|target| {
                let bytes = format!("{name}-{version}-{target}");
                Artifact {
                    target: target.parse().unwrap(),
                    sha256: sha(bytes.as_bytes()),
                    size: bytes.len() as i64,
                }
            })
            .collect();
        self.sign_and_publish(token, name, version, artifacts)
            .await
            .0
    }

    async fn sign_and_publish(
        &self,
        token: &str,
        name: &str,
        version: &str,
        artifacts: Vec<Artifact>,
    ) -> (u16, Value) {
        let key = signing::generate();
        let (status, certificate) = self
            .call(
                "POST",
                "/api/v1/signing/certificate",
                Some(token),
                Some(json!({
                    "ecosystem": "cli",
                    "package": name,
                    "public_key": signing::encode_key(key.verifying_key()),
                })),
            )
            .await;
        if status != 200 {
            return (status, certificate);
        }

        let statement = signing::sign(
            &key,
            &Statement {
                package: name.to_owned(),
                version: version.to_owned(),
                artifacts,
                permissions: None,
                completions: None,
                env: Default::default(),
            },
        );
        self.call(
            "POST",
            &format!("/cli/v1/packages/{name}/versions/{version}/publish"),
            Some(token),
            Some(json!({ "statement": statement, "certificate": certificate })),
        )
        .await
    }

    async fn root_keys(&self) -> Vec<p256::ecdsa::VerifyingKey> {
        let (status, keys) = self.call("GET", "/api/v1/signing/keys", None, None).await;
        assert_eq!(status, 200);
        keys.as_array()
            .unwrap()
            .iter()
            .map(|k| {
                signing::verifying_key_from_pem(k["public_key_pem"].as_str().unwrap()).unwrap()
            })
            .collect()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[sqlx::test(migrations = "./migrations")]
async fn publish_then_install_round_trip(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;

    assert_eq!(registry.publish(&registry.admin, "ff", "0.1.0").await, 200);
    assert_eq!(registry.publish(&registry.admin, "ff", "0.10.0").await, 200);

    let reader = registry
        .kanidm
        .user("friend", &["registry_users@idm.example.test"]);

    let (status, body) = registry
        .call("GET", "/cli/v1/packages", Some(&reader), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body[0]["name"], "ff");
    assert_eq!(body[0]["latest"], "0.10.0");
    assert_eq!(body[0]["aliases"], json!(["ffctl"]));

    let (status, latest) = registry
        .call(
            "GET",
            "/cli/v1/packages/ff/latest?target=darwin-arm64",
            Some(&reader),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(latest["version"], "0.10.0");
    assert_eq!(latest["published_by"], "anurag");
    assert_eq!(latest["auth"]["client_id"], "ff");
    assert_eq!(latest["auth"]["token_env"], "FFCTL_TOKEN");
    assert_eq!(latest["auth"]["scopes"], "openid email profile groups");

    let (status, many) = registry
        .call(
            "POST",
            "/cli/v1/latest",
            Some(&reader),
            Some(json!({ "target": "darwin-arm64", "names": ["ff", "missing"] })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(many, json!({ "ff": "0.10.0" }));

    let expected = b"ff-0.10.0-darwin-arm64";
    let (status, bytes, header) = registry
        .download(
            &reader,
            "/cli/v1/packages/ff/versions/0.10.0/artifacts/darwin-arm64",
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(bytes, expected);
    assert_eq!(header.as_deref(), Some(sha(expected).as_str()));
    assert_eq!(latest["artifacts"][0]["sha256"], sha(expected));
}

#[sqlx::test(migrations = "./migrations")]
async fn publishing_latest_updates_package_description(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;

    for (version, description, expected) in [
        ("1.0.0", Some("first"), "first"),
        ("0.9.0", Some("older"), "first"),
        ("2.0.0-rc.1", Some("prerelease"), "first"),
        ("1.1.0", Some(""), "first"),
        ("1.2.0", None, "first"),
        ("1.3.0", Some("second"), "second"),
    ] {
        let status = registry
            .publish_described(&registry.admin, "ff", version, description)
            .await;
        assert_eq!(status, 200, "{version}");

        let (status, body) = registry
            .call("GET", "/cli/v1/packages/ff", Some(&registry.admin), None)
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["description"], expected, "{version}");
    }

    let (status, body) = registry
        .call(
            "GET",
            "/cli/v1/packages?q=second",
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body[0]["name"], "ff");
}

#[sqlx::test(migrations = "./migrations")]
async fn archived_packages_leave_search_but_stay_installable(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    assert_eq!(registry.publish(&registry.admin, "ff", "1.0.0").await, 200);

    let names = |body: &serde_json::Value| body.as_array().unwrap().len();
    let (_, body) = registry
        .call(
            "GET",
            "/cli/v1/packages",
            Some(registry.admin.as_str()),
            None,
        )
        .await;
    assert_eq!(names(&body), 1);

    let (status, body) = registry
        .call(
            "POST",
            "/cli/v1/packages/ff/archive",
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["archived"], true);

    let (_, body) = registry
        .call(
            "GET",
            "/cli/v1/packages",
            Some(registry.admin.as_str()),
            None,
        )
        .await;
    assert_eq!(names(&body), 0);
    let (_, body) = registry
        .call(
            "GET",
            "/cli/v1/packages?q=ff",
            Some(registry.admin.as_str()),
            None,
        )
        .await;
    assert_eq!(names(&body), 0);

    let (status, body) = registry
        .call(
            "GET",
            "/cli/v1/packages/ff/latest?target=darwin-arm64",
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = registry
        .call("GET", "/cli/v1/packages/ff", Some(&registry.admin), None)
        .await;
    assert_eq!(status, 200);

    assert_eq!(registry.publish(&registry.admin, "ff", "1.1.0").await, 409);

    let (status, body) = registry
        .call(
            "POST",
            "/cli/v1/packages/ff/unarchive",
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["archived"], false);
    assert_eq!(registry.publish(&registry.admin, "ff", "1.1.0").await, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_admins_can_archive(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    let reader = registry
        .kanidm
        .user("friend", &["registry_users@idm.example.test"]);
    let (status, _) = registry
        .call("POST", "/cli/v1/packages/ff/archive", Some(&reader), None)
        .await;
    assert_eq!(status, 403);
}

#[sqlx::test(migrations = "./migrations")]
async fn published_versions_are_immutable(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    assert_eq!(registry.publish(&registry.admin, "ff", "1.0.0").await, 200);

    assert_eq!(registry.publish(&registry.admin, "ff", "1.0.0").await, 409);
    let (status, _) = registry
        .upload(
            &registry.admin,
            "/cli/v1/packages/ff/versions/1.0.0/artifacts/linux-amd64",
            b"evil",
            &sha(b"evil"),
        )
        .await;
    assert_eq!(status, 409);
}

#[sqlx::test(migrations = "./migrations")]
async fn publish_requires_every_declared_target(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    let admin = registry.admin.clone();

    let base = "/cli/v1/packages/ff/versions";
    registry
        .call(
            "POST",
            base,
            Some(&admin),
            Some(json!({ "version": "1.0.0", "targets": ["darwin-arm64", "linux-amd64"] })),
        )
        .await;
    registry
        .upload(
            &admin,
            &format!("{base}/1.0.0/artifacts/linux-amd64"),
            b"x",
            &sha(b"x"),
        )
        .await;

    let (status, body) = registry
        .sign_and_publish(
            &admin,
            "ff",
            "1.0.0",
            vec![Artifact {
                target: Target::LinuxAmd64,
                sha256: sha(b"x"),
                size: 1,
            }],
        )
        .await;
    assert_eq!(status, 422);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("darwin-arm64")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn uploads_are_checked_against_their_digest_and_size(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    let admin = registry.admin.clone();

    let base = "/cli/v1/packages/ff/versions";
    registry
        .call(
            "POST",
            base,
            Some(&admin),
            Some(json!({ "version": "1.0.0", "targets": ["linux-amd64"] })),
        )
        .await;
    let path = format!("{base}/1.0.0/artifacts/linux-amd64");

    let (status, _) = registry
        .upload(&admin, &path, b"actual", &sha(b"claimed"))
        .await;
    assert_eq!(status, 422);

    let big = vec![0u8; 2048];
    let (status, _) = registry.upload(&admin, &path, &big, &sha(&big)).await;
    assert_eq!(status, 413);

    let (status, _) = registry
        .upload(
            &admin,
            &format!("{base}/1.0.0/artifacts/darwin-arm64"),
            b"x",
            &sha(b"x"),
        )
        .await;
    assert_eq!(status, 400, "undeclared target");

    let (status, _) = registry
        .call(
            "POST",
            base,
            Some(&admin),
            Some(json!({ "version": "2.0.0", "targets": ["linux-amd64"], "binary_name": "../../.bashrc" })),
        )
        .await;
    assert_eq!(status, 400, "path traversal in binary_name");
}

#[sqlx::test(migrations = "./migrations")]
async fn yanked_versions_drop_out_of_latest(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    let admin = registry.admin.clone();
    registry.publish(&admin, "ff", "1.0.0").await;
    registry.publish(&admin, "ff", "1.1.0").await;

    let (status, _) = registry
        .call(
            "POST",
            "/cli/v1/packages/ff/versions/1.1.0/yank",
            Some(&admin),
            Some(json!({ "reason": "broken" })),
        )
        .await;
    assert_eq!(status, 200);

    let (_, latest) = registry
        .call("GET", "/cli/v1/packages/ff/latest", Some(&admin), None)
        .await;
    assert_eq!(latest["version"], "1.0.0");

    let (status, _, _) = registry
        .download(
            &admin,
            "/cli/v1/packages/ff/versions/1.1.0/artifacts/linux-amd64",
        )
        .await;
    assert_eq!(status, 200);

    registry
        .call(
            "POST",
            "/cli/v1/packages/ff/versions/1.1.0/unyank",
            Some(&admin),
            None,
        )
        .await;
    let (_, latest) = registry
        .call("GET", "/cli/v1/packages/ff/latest", Some(&admin), None)
        .await;
    assert_eq!(latest["version"], "1.1.0");
}

#[sqlx::test(migrations = "./migrations")]
async fn people_need_registry_groups(pool: PgPool) {
    let registry = Registry::spawn(pool).await;

    let (status, _) = registry.call("GET", "/cli/v1/packages", None, None).await;
    assert_eq!(status, 401);

    let outsider = registry
        .kanidm
        .user("mallory", &["grafana_admins@idm.example.test"]);
    let (status, _) = registry
        .call("GET", "/cli/v1/packages", Some(&outsider), None)
        .await;
    assert_eq!(status, 403);

    let reader = registry.kanidm.user("friend", &["registry_users"]);
    let (status, _) = registry
        .call("GET", "/cli/v1/packages", Some(&reader), None)
        .await;
    assert_eq!(status, 200);

    let (status, _) = registry
        .call(
            "POST",
            "/cli/v1/packages",
            Some(&reader),
            Some(json!({ "name": "ff" })),
        )
        .await;
    assert_eq!(status, 403);

    registry.create_package("ff").await;
    assert_eq!(registry.publish(&reader, "ff", "1.0.0").await, 403);

    let (status, me) = registry
        .call("GET", "/api/v1/whoami", Some(&registry.admin), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(me["subject"], "anurag");
    assert_eq!(me["admin"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn reserved_and_invalid_names_are_refused(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    for name in ["install", "Bad", "has_underscore"] {
        let (status, _) = registry
            .call(
                "POST",
                "/cli/v1/packages",
                Some(&registry.admin),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(status, 400, "{name}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn publish_tokens_are_scoped_and_revocable(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    registry.create_package("ai").await;

    let (status, created) = registry
        .call(
            "POST",
            "/api/v1/tokens",
            Some(&registry.admin),
            Some(json!({ "name": "ff-ci", "scopes": [{ "ecosystem": "cli", "package": "ff" }] })),
        )
        .await;
    assert_eq!(status, 201);
    let token = created["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("ortzi_"));

    assert_eq!(registry.publish(&token, "ff", "1.0.0").await, 200);
    assert_eq!(registry.publish(&token, "ai", "1.0.0").await, 403);

    let (_, version) = registry
        .call(
            "GET",
            "/cli/v1/packages/ff/versions/1.0.0",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(version["published_by"], "token:ff-ci");

    let (_, tokens) = registry
        .call("GET", "/api/v1/tokens", Some(&registry.admin), None)
        .await;
    assert_eq!(
        tokens[0]["scopes"],
        json!([{ "ecosystem": "cli", "package": "ff" }])
    );
    assert!(tokens[0].get("token").is_none());

    let id = created["id"].as_str().unwrap();
    let (status, _) = registry
        .call(
            "DELETE",
            &format!("/api/v1/tokens/{id}"),
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(registry.publish(&token, "ff", "1.0.1").await, 401);

    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/tokens",
            Some(&registry.admin),
            Some(json!({ "name": "x", "scopes": [{ "ecosystem": "cli", "package": "nope" }] })),
        )
        .await;
    assert_eq!(status, 404);
}

#[sqlx::test(migrations = "./migrations")]
async fn forgejo_trusted_publishing(pool: PgPool) {
    let registry = Registry::spawn(pool.clone()).await;
    registry.create_package("tool").await;

    let ci = registry.forgejo.forgejo(json!({}));

    assert_eq!(registry.publish(&ci, "tool", "1.0.0").await, 403);

    let (status, publisher) = registry
        .call(
            "POST",
            "/api/v1/trusted-publishers",
            Some(&registry.admin),
            Some(json!({
                "package": "tool",
                "repository": "anurag/tools",
                "ref_pattern": "refs/heads/main",
                "workflow": "publish.yaml",
            })),
        )
        .await;
    assert_eq!(status, 201, "{publisher}");
    assert!(publisher["repository_id"].is_null());

    assert_eq!(registry.publish(&ci, "tool", "1.0.0").await, 200);

    let (_, version) = registry
        .call(
            "GET",
            "/cli/v1/packages/tool/versions/1.0.0",
            Some(&ci),
            None,
        )
        .await;
    assert_eq!(version["published_by"], "forgejo:anurag/tools");
    assert_eq!(version["source"]["sha"], "deadbeef");
    assert_eq!(version["source"]["run_id"], "7");

    let (_, publishers) = registry
        .call(
            "GET",
            "/api/v1/trusted-publishers",
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(publishers[0]["repository_id"], "42");

    let rejected = [
        json!({ "repository": "anurag/other", "workflow_ref": "anurag/other/.forgejo/workflows/publish.yaml@refs/heads/main" }),
        json!({ "repository_id": "43" }),
        json!({ "ref": "refs/heads/feature" }),
        json!({ "workflow_ref": "anurag/tools/.forgejo/workflows/other.yaml@refs/heads/main" }),
        json!({ "event_name": "pull_request" }),
    ];
    for (i, overrides) in rejected.into_iter().enumerate() {
        let token = registry.forgejo.forgejo(overrides.clone());
        let status = registry.publish(&token, "tool", &format!("1.1.{i}")).await;
        assert_eq!(status, 403, "{overrides}");
    }

    let wrong_audience = registry.forgejo.forgejo(json!({ "aud": "something-else" }));
    assert_eq!(
        registry.publish(&wrong_audience, "tool", "1.2.0").await,
        401
    );

    let kanidm_as_ci = registry
        .kanidm
        .sign(json!({ "aud": FORGEJO_AUDIENCE, "repository": "anurag/tools" }));
    assert_eq!(registry.publish(&kanidm_as_ci, "tool", "1.3.0").await, 401);
}

#[sqlx::test(migrations = "./migrations")]
async fn unpublished_packages_can_be_deleted(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("typo").await;
    registry.create_package("ff").await;
    registry.publish(&registry.admin, "ff", "1.0.0").await;

    let (status, _) = registry
        .call(
            "DELETE",
            "/cli/v1/packages/typo",
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, _) = registry
        .call("DELETE", "/cli/v1/packages/ff", Some(&registry.admin), None)
        .await;
    assert_eq!(status, 409);

    let (_, audit) = registry
        .call(
            "GET",
            "/api/v1/audit?package=typo",
            Some(&registry.admin),
            None,
        )
        .await;
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["package.delete", "package.create"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn releases_are_signed_end_to_end(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("ff").await;
    registry.create_package("ai").await;
    let admin = registry.admin.clone();
    assert_eq!(registry.publish(&admin, "ff", "1.0.0").await, 200);

    let (_, version) = registry
        .call(
            "GET",
            "/cli/v1/packages/ff/versions/1.0.0",
            Some(&admin),
            None,
        )
        .await;
    let signatures = serde_json::from_value(version["signatures"].clone()).unwrap();
    let roots = registry.root_keys().await;
    let manifest = signing::verify_release(&roots, "ff", "1.0.0", &signatures).unwrap();
    assert_eq!(manifest.published_by, "anurag");
    assert_eq!(
        manifest.statement.artifacts[0].sha256,
        sha(b"ff-1.0.0-darwin-arm64")
    );
    assert!(signing::verify_release(&roots, "ai", "1.0.0", &signatures).is_err());

    let base = "/cli/v1/packages/ff/versions";
    registry
        .call(
            "POST",
            base,
            Some(&admin),
            Some(json!({ "version": "2.0.0", "targets": ["linux-amd64"] })),
        )
        .await;
    registry
        .upload(
            &admin,
            &format!("{base}/2.0.0/artifacts/linux-amd64"),
            b"real",
            &sha(b"real"),
        )
        .await;
    let claimed = |sha256: String| {
        vec![Artifact {
            target: Target::LinuxAmd64,
            sha256,
            size: 4,
        }]
    };

    let (status, _) = registry
        .sign_and_publish(&admin, "ff", "2.0.0", claimed(sha(b"fake")))
        .await;
    assert_eq!(status, 422, "statement must match the uploaded sha");

    let key = signing::generate();
    let (_, other_package) = registry
        .call(
            "POST",
            "/api/v1/signing/certificate",
            Some(&admin),
            Some(json!({ "ecosystem": "cli", "package": "ai", "public_key": signing::encode_key(key.verifying_key()) })),
        )
        .await;
    let statement = signing::sign(
        &key,
        &Statement {
            package: "ff".to_owned(),
            version: "2.0.0".to_owned(),
            artifacts: claimed(sha(b"real")),
            permissions: None,
            completions: None,
            env: Default::default(),
        },
    );
    let (status, _) = registry
        .call(
            "POST",
            &format!("{base}/2.0.0/publish"),
            Some(&admin),
            Some(json!({ "statement": statement, "certificate": other_package })),
        )
        .await;
    assert_eq!(status, 400, "certificate for another package");

    let forged = signing::sign(
        &signing::generate(),
        &json!({ "ecosystem": "cli", "package": "ff" }),
    );
    let (status, _) = registry
        .call(
            "POST",
            &format!("{base}/2.0.0/publish"),
            Some(&admin),
            Some(json!({ "statement": statement, "certificate": forged })),
        )
        .await;
    assert_eq!(status, 400, "certificate not signed by the registry");

    let (status, _) = registry
        .sign_and_publish(&admin, "ff", "2.0.0", claimed(sha(b"real")))
        .await;
    assert_eq!(status, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn harbor_token_exchange(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    let pull = json!({ "access": [{ "project": "containers", "actions": ["pull"] }] });
    let push = json!({ "access": [{ "project": "containers", "actions": ["push"] }] });

    let ci = registry.forgejo.forgejo(json!({}));
    let (status, token) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&ci),
            Some(pull.clone()),
        )
        .await;
    assert_eq!(status, 200, "{token}");
    assert_eq!(token["registry"], "registry.anurag.sh");
    assert_eq!(token["password"], "secret-1");
    let username = token["username"].as_str().unwrap().to_owned();
    assert!(
        username.starts_with("robot$exch-forgejo-forgejo-anurag-tools-"),
        "{username}"
    );

    let created = registry.harbor.calls.created.lock().unwrap()[0].clone();
    assert_eq!(created["duration"], 1);
    assert_eq!(created["description"], "forgejo:anurag/tools run 7");
    assert_eq!(
        created["permissions"],
        json!([{
            "kind": "project",
            "namespace": "containers",
            "access": [{ "resource": "repository", "action": "pull" }],
        }])
    );

    let (status, body) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&ci),
            Some(push.clone()),
        )
        .await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no push grant on harbor project containers")
    );

    let k8s_main = registry.forgejo.forgejo(json!({
        "repository": "anurag/k8s",
        "workflow_ref": "anurag/k8s/.forgejo/workflows/build.yaml@refs/heads/main",
    }));
    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&k8s_main),
            Some(push.clone()),
        )
        .await;
    assert_eq!(status, 200);

    let pull_request = registry
        .forgejo
        .forgejo(json!({ "event_name": "pull_request" }));
    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&pull_request),
            Some(pull.clone()),
        )
        .await;
    assert_eq!(status, 403);

    let reader = registry
        .kanidm
        .user("reader", &["registry_users@idm.example.test"]);
    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&reader),
            Some(json!({ "access": [{ "project": "ortzi-catalog", "actions": ["pull"] }] })),
        )
        .await;
    assert_eq!(status, 200);

    let path = format!("/api/v1/harbor/token/{}", username.replace('$', "%24"));
    let (status, _) = registry.call("DELETE", &path, Some(&reader), None).await;
    assert_eq!(status, 403);
    let (status, _) = registry.call("DELETE", &path, Some(&ci), None).await;
    assert_eq!(status, 204);
    assert_eq!(*registry.harbor.calls.deleted.lock().unwrap(), vec![1]);
    let (status, _) = registry.call("DELETE", &path, Some(&ci), None).await;
    assert_eq!(status, 404);
}

#[sqlx::test(migrations = "./migrations")]
async fn harbor_token_for_ortzi_publishing(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("tool").await;
    let request = json!({
        "access": [{ "project": "ortzi-packages", "actions": ["push"], "package": "tool" }]
    });

    let ci = registry.forgejo.forgejo(json!({}));
    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&ci),
            Some(request.clone()),
        )
        .await;
    assert_eq!(status, 403);

    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/trusted-publishers",
            Some(&registry.admin),
            Some(json!({ "package": "tool", "repository": "anurag/tools" })),
        )
        .await;
    assert_eq!(status, 201);

    let (status, token) = registry
        .call("POST", "/api/v1/harbor/token", Some(&ci), Some(request))
        .await;
    assert_eq!(status, 200, "{token}");

    let (status, _) = registry
        .call(
            "POST",
            "/api/v1/harbor/token",
            Some(&ci),
            Some(json!({
                "access": [{ "project": "containers", "actions": ["pull"], "package": "tool" }]
            })),
        )
        .await;
    assert_eq!(status, 400);
}

#[sqlx::test(migrations = "./migrations")]
async fn harbor_token_as_docker_config(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    let ci = registry.forgejo.forgejo(json!({}));

    let response = registry
        .http
        .post(format!("{}/api/v1/harbor/token", registry.url))
        .bearer_auth(&ci)
        .header("accept", "application/vnd.docker.config+json")
        .json(&json!({ "access": [{ "project": "containers", "actions": ["pull"] }] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let config: Value = response.json().await.unwrap();
    let auth = config["auths"]["registry.anurag.sh"]["auth"]
        .as_str()
        .unwrap();
    let decoded = String::from_utf8(
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, auth).unwrap(),
    )
    .unwrap();
    assert!(decoded.starts_with("robot$exch-forgejo-"), "{decoded}");
    assert!(decoded.ends_with(":secret-1"), "{decoded}");
}

fn fetch_catalog(registry: &Registry) -> registry_api::Catalog {
    let calls = &registry.harbor.calls;
    let manifest = calls
        .manifest("ortzi-catalog/catalog", "latest")
        .expect("catalog is published");
    let layer = manifest["layers"][0]["digest"].as_str().unwrap();
    let envelope: signing::Envelope = serde_json::from_slice(&calls.blob(layer).unwrap()).unwrap();
    signing::verify_catalog(&[registry.signer.verifying_key()], &envelope).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn publishing_releases_into_harbor_with_a_signed_catalog(pool: PgPool) {
    let registry = Registry::spawn(pool).await;
    registry.create_package("tool").await;
    assert_eq!(
        registry.publish(&registry.admin, "tool", "1.0.0").await,
        200
    );

    let calls = &registry.harbor.calls;
    let index = calls.manifest("ortzi-packages/tool", "1.0.0").unwrap();
    assert_eq!(index["mediaType"], registry_api::oci::INDEX);
    assert_eq!(index["artifactType"], registry_api::PACKAGE_ARTIFACT);
    let mut platforms: Vec<String> = index["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            format!(
                "{}/{}",
                m["platform"]["os"].as_str().unwrap(),
                m["platform"]["architecture"].as_str().unwrap()
            )
        })
        .collect();
    platforms.sort();
    assert_eq!(platforms, ["darwin/arm64", "linux/amd64"]);

    let linux = index["manifests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["platform"]["os"] == "linux")
        .unwrap();
    let binary = calls
        .manifest("ortzi-packages/tool", linux["digest"].as_str().unwrap())
        .unwrap();
    let layer = &binary["layers"][0];
    assert_eq!(
        layer["annotations"]["org.opencontainers.image.title"],
        "tool"
    );
    assert_eq!(
        calls.blob(layer["digest"].as_str().unwrap()).unwrap(),
        b"tool-1.0.0-linux-amd64"
    );

    let catalog = fetch_catalog(&registry);
    let entry = &catalog.packages["tool"].versions["1.0.0"];
    let index_body = calls.manifests.lock().unwrap()["ortzi-packages/tool@1.0.0"]
        .1
        .clone();
    assert_eq!(entry.index, oci_digest(&index_body));

    let signatures = calls
        .manifest("ortzi-packages/tool", &entry.signatures)
        .unwrap();
    assert_eq!(signatures["subject"]["digest"], entry.index);
    let signatures: signing::Signatures = serde_json::from_slice(
        &calls
            .blob(signatures["layers"][0]["digest"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let release = signing::verify_release(
        &[registry.signer.verifying_key()],
        "tool",
        "1.0.0",
        &signatures,
    )
    .unwrap();
    assert_eq!(release.statement.artifacts.len(), 2);
    assert_eq!(
        catalog
            .latest("tool", Some(Target::LinuxAmd64), false)
            .map(|(v, _)| v),
        Some("1.0.0")
    );

    let seq = catalog.seq;
    let (status, _) = registry
        .call(
            "POST",
            "/cli/v1/packages/tool/versions/1.0.0/yank",
            Some(&registry.admin),
            Some(json!({ "reason": "broken" })),
        )
        .await;
    assert_eq!(status, 200);
    let catalog = fetch_catalog(&registry);
    assert!(catalog.seq > seq);
    assert_eq!(
        catalog.packages["tool"].versions["1.0.0"]
            .yanked
            .as_ref()
            .unwrap()
            .reason
            .as_deref(),
        Some("broken")
    );
    assert!(catalog.latest("tool", None, false).is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn oci_pushed_artifacts_register_publish_and_download(pool: PgPool) {
    use registry_api::{BINARY_ARTIFACT, oci};

    let registry = Registry::spawn(pool).await;
    registry.create_package("tool").await;
    let base = "/cli/v1/packages/tool/versions";
    let (status, _) = registry
        .call(
            "POST",
            base,
            Some(&registry.admin),
            Some(json!({ "version": "2.0.0", "targets": ["linux-amd64", "wasm"] })),
        )
        .await;
    assert_eq!(status, 201);

    let client = oci::Client::new(reqwest::Client::new(), &registry.harbor.url, None).unwrap();
    let chunked = oci::Client::new(reqwest::Client::new(), &registry.harbor.url, None)
        .unwrap()
        .with_chunk_size(4);
    let repository = "ortzi-packages/tool";
    let mut artifacts = Vec::new();
    for target in [Target::LinuxAmd64, Target::Wasm] {
        let bytes = format!("oci-{target}").into_bytes();
        let path =
            std::env::temp_dir().join(format!("registry-test-{}-{target}", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let layer = chunked
            .push_file(repository, target.media_type(), &path)
            .await
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        client
            .push_bytes(repository, oci::EMPTY, oci::EMPTY_JSON)
            .await
            .unwrap();
        let manifest = client
            .push_json(
                repository,
                &oci_digest(
                    &serde_json::to_vec(&oci::Manifest::artifact(
                        BINARY_ARTIFACT,
                        vec![layer.clone()],
                    ))
                    .unwrap(),
                ),
                oci::MANIFEST,
                &oci::Manifest::artifact(BINARY_ARTIFACT, vec![layer]),
            )
            .await
            .unwrap();

        let (status, body) = registry
            .call(
                "PUT",
                &format!("{base}/2.0.0/artifacts/{target}/manifest"),
                Some(&registry.admin),
                Some(json!({ "manifest": manifest.digest })),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["sha256"], sha(&bytes));
        artifacts.push(Artifact {
            target,
            sha256: sha(&bytes),
            size: bytes.len() as i64,
        });
    }

    let wrong = client
        .push_json(
            repository,
            &oci_digest(
                &serde_json::to_vec(&oci::Manifest::artifact("application/vnd.other", vec![]))
                    .unwrap(),
            ),
            oci::MANIFEST,
            &oci::Manifest::artifact("application/vnd.other", vec![]),
        )
        .await
        .unwrap();
    let (status, _) = registry
        .call(
            "PUT",
            &format!("{base}/2.0.0/artifacts/linux-amd64/manifest"),
            Some(&registry.admin),
            Some(json!({ "manifest": wrong.digest })),
        )
        .await;
    assert_eq!(status, 422);

    let (status, body) = registry
        .sign_and_publish(&registry.admin, "tool", "2.0.0", artifacts)
        .await;
    assert_eq!(status, 200, "{body}");

    let (status, bytes, sha256) = registry
        .download(
            &registry.admin,
            &format!("{base}/2.0.0/artifacts/linux-amd64"),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(bytes, b"oci-linux-amd64");
    assert_eq!(sha256.as_deref(), Some(sha(b"oci-linux-amd64").as_str()));

    let (status, _) = registry
        .call(
            "GET",
            &format!("{base}/2.0.0/artifacts/linux-amd64/url"),
            Some(&registry.admin),
            None,
        )
        .await;
    assert_eq!(status, 404);

    let catalog = fetch_catalog(&registry);
    assert_eq!(
        catalog
            .latest("tool", Some(Target::DarwinArm64), false)
            .map(|(v, _)| v),
        Some("2.0.0")
    );
}
