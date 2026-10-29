use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use registry::{
    config::{Config, ForgejoConfig},
    server,
    state::AppState,
    storage::Storage,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const KANIDM_AUDIENCE: &str = "registry";
const FORGEJO_AUDIENCE: &str = "registry.ortzi.cloud";

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

struct Registry {
    url: String,
    http: reqwest::Client,
    handle: JoinHandle<()>,
    kanidm: Idp,
    forgejo: Idp,
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
            audience: FORGEJO_AUDIENCE.to_owned(),
            jwks_uri: None,
        });
        config.limits.max_artifact_bytes = 1024;

        let state = AppState::new(config, pool, Storage::memory());
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
        let base = format!("/cli/v1/packages/{name}/versions");
        let (status, body) = self
            .call(
                "POST",
                &base,
                Some(token),
                Some(json!({
                    "version": version,
                    "targets": ["darwin-arm64", "linux-amd64"],
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

        self.call(
            "POST",
            &format!("{base}/{version}/publish"),
            Some(token),
            None,
        )
        .await
        .0
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
        .call("POST", &format!("{base}/1.0.0/publish"), Some(&admin), None)
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
