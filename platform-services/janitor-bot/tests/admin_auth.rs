use axum::{Router, middleware, routing::get};
use janitor_bot::auth::{self, Authenticator, OAuthConfig};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use reqwest::StatusCode;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const KID: &str = "test-key";
const AUDIENCE: &str = "janitor-bot";
const ADMIN_GROUP: &str = "janitor_bot_admins";

struct Idp {
    issuer: String,
    key: EncodingKey,
    handle: JoinHandle<()>,
}

impl Idp {
    async fn spawn(userinfo_groups: &[&str]) -> Self {
        let secret = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
        let key = EncodingKey::from_ec_der(secret.to_pkcs8_der().unwrap().as_bytes());

        let mut jwk: serde_json::Value =
            serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(KID);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");

        let jwks = json!({ "keys": [jwk] }).to_string();
        let userinfo = json!({ "groups": userinfo_groups }).to_string();

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
            key,
            handle,
        }
    }

    fn authenticator(&self, groups: &[&str]) -> Authenticator {
        Authenticator::new(&OAuthConfig {
            issuer: self.issuer.clone(),
            audience: Some(AUDIENCE.to_owned()),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..OAuthConfig::default()
        })
    }

    fn token(&self, issuer: &str, audience: &str, expires_in: i64) -> String {
        self.sign(issuer, audience, expires_in, None)
    }

    fn token_with_groups(&self, groups: &[&str]) -> String {
        self.sign(&self.issuer, AUDIENCE, 300, Some(groups))
    }

    fn sign(
        &self,
        issuer: &str,
        audience: &str,
        expires_in: i64,
        groups: Option<&[&str]>,
    ) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(KID.to_owned());

        let mut claims = json!({
            "iss": issuer,
            "aud": audience,
            "sub": "00000000-0000-0000-0000-000000000001",
            "preferred_username": "anurag",
            "exp": chrono::Utc::now().timestamp() + expires_in,
        });

        if let Some(groups) = groups {
            claims["groups"] = json!(groups);
        }

        encode(&header, &claims, &self.key).unwrap()
    }
}

impl Drop for Idp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// A router with one guarded admin route and the unguarded metrics route, laid out like
/// janitor-bot's.
struct Bot {
    url: String,
    http: reqwest::Client,
    handle: JoinHandle<()>,
}

impl Bot {
    async fn spawn(auth: Option<Authenticator>) -> Self {
        let admin = Router::new()
            .route("/admin/rules", get(|| async { "rules" }))
            .route_layer(middleware::from_fn_with_state(auth, auth::require_admin));
        let app = Router::new()
            .route("/admin/metrics", get(|| async { "metrics" }))
            .merge(admin);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            url,
            http: reqwest::Client::new(),
            handle,
        }
    }

    async fn get(&self, path: &str, token: Option<&str>) -> StatusCode {
        let mut request = self.http.get(format!("{}{path}", self.url));

        if let Some(token) = token {
            request = request.bearer_auth(token);
        }

        request.send().await.unwrap().status()
    }
}

impl Drop for Bot {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[tokio::test]
async fn admin_rejects_missing_and_invalid_tokens() {
    let idp = Idp::spawn(&[ADMIN_GROUP]).await;
    let bot = Bot::spawn(Some(idp.authenticator(&[]))).await;

    assert_eq!(
        bot.get("/admin/rules", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        bot.get("/admin/rules", Some("not-a-jwt")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        bot.get(
            "/admin/rules",
            Some(&idp.token("https://evil.test", AUDIENCE, 300))
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        bot.get(
            "/admin/rules",
            Some(&idp.token(&idp.issuer, "ai-gateway", 300))
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        bot.get(
            "/admin/rules",
            Some(&idp.token(&idp.issuer, AUDIENCE, -600))
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn admin_accepts_a_valid_jwt() {
    let idp = Idp::spawn(&[]).await;
    let bot = Bot::spawn(Some(idp.authenticator(&[]))).await;

    assert_eq!(
        bot.get("/admin/rules", Some(&idp.token(&idp.issuer, AUDIENCE, 300)))
            .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_requires_a_configured_group_from_the_token() {
    let idp = Idp::spawn(&[]).await;
    let bot = Bot::spawn(Some(idp.authenticator(&[ADMIN_GROUP]))).await;

    assert_eq!(
        bot.get("/admin/rules", Some(&idp.token_with_groups(&[ADMIN_GROUP])))
            .await,
        StatusCode::OK
    );
    assert_eq!(
        bot.get(
            "/admin/rules",
            Some(&idp.token_with_groups(&["someone_else"]))
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn admin_falls_back_to_userinfo_groups() {
    let idp = Idp::spawn(&["janitor_bot_admins@idm.example.test"]).await;
    let bot = Bot::spawn(Some(idp.authenticator(&[ADMIN_GROUP]))).await;

    assert_eq!(
        bot.get("/admin/rules", Some(&idp.token(&idp.issuer, AUDIENCE, 300)))
            .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn metrics_stay_open_for_prometheus() {
    let idp = Idp::spawn(&[]).await;
    let bot = Bot::spawn(Some(idp.authenticator(&[ADMIN_GROUP]))).await;

    assert_eq!(bot.get("/admin/metrics", None).await, StatusCode::OK);
}

#[tokio::test]
async fn admin_is_open_without_an_authenticator() {
    let bot = Bot::spawn(None).await;

    assert_eq!(bot.get("/admin/rules", None).await, StatusCode::OK);
}
