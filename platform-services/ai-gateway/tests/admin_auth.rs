use ai_gateway::{
    auth::Authenticator,
    config::{Config, OAuthConfig},
    feature_flag::FeatureFlagClient,
    pricing::Pricing,
    providers::Registry,
    server,
    state::AppState,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const KID: &str = "test-key";
const AUDIENCE: &str = "ai-gateway";
const ADMIN_TOKEN: &str = "static-admin-token";
const ADMIN_GROUP: &str = "ai_gateway_admins";

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

struct Gateway {
    url: String,
    http: reqwest::Client,
    handle: JoinHandle<()>,
}

impl Gateway {
    async fn spawn(pool: PgPool, auth: Option<Authenticator>) -> Self {
        let config = Config {
            admin_token: ADMIN_TOKEN.to_owned(),
            ..Config::default()
        };
        let registry = Registry::from_config(&config);
        let features = FeatureFlagClient::new(None).await;
        let state = AppState::new(
            config,
            registry,
            pool,
            features,
            Pricing::default(),
            None,
            auth,
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = server::router(state);

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            url,
            http: reqwest::Client::new(),
            handle,
        }
    }

    async fn list_keys(&self, token: Option<&str>) -> u16 {
        let mut request = self.http.get(format!("{}/admin/keys", self.url));

        if let Some(token) = token {
            request = request.bearer_auth(token);
        }

        request.send().await.unwrap().status().as_u16()
    }

    async fn usage(&self, query: &str) -> (u16, serde_json::Value) {
        let response = self
            .http
            .get(format!("{}/admin/usage{query}", self.url))
            .bearer_auth(ADMIN_TOKEN)
            .send()
            .await
            .unwrap();

        let status = response.status().as_u16();

        (status, response.json().await.unwrap())
    }
}

async fn record_usage(pool: &PgPool, key: &str, model: &str, age: &str, cache_hit: bool) {
    sqlx::query(
        "INSERT INTO usage_events \
         (key_name, provider, requested_model, resolved_model, input_tokens, output_tokens, \
          cost_usd, cache_hit, created_at) \
         VALUES ($1, 'test', $2, $2, 100, 10, 0.5, $3, now() - $4::interval)",
    )
    .bind(key)
    .bind(model)
    .bind(cache_hit)
    .bind(age)
    .execute(pool)
    .await
    .unwrap();
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_accepts_the_static_token_and_a_valid_jwt(pool: PgPool) {
    let idp = Idp::spawn(&[]).await;
    let gateway = Gateway::spawn(pool, Some(idp.authenticator(&[]))).await;

    assert_eq!(gateway.list_keys(Some(ADMIN_TOKEN)).await, 200);

    let token = idp.token(&idp.issuer, AUDIENCE, 300);
    assert_eq!(gateway.list_keys(Some(&token)).await, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_requires_a_configured_group_from_the_token(pool: PgPool) {
    let idp = Idp::spawn(&[ADMIN_GROUP]).await;
    let gateway = Gateway::spawn(pool, Some(idp.authenticator(&[ADMIN_GROUP]))).await;

    let member = idp.token_with_groups(&["ai_gateway_admins@idm.example.test"]);
    assert_eq!(gateway.list_keys(Some(&member)).await, 200);

    let outsider = idp.token_with_groups(&["grafana_admins@idm.example.test"]);
    assert_eq!(gateway.list_keys(Some(&outsider)).await, 403);

    let groupless = idp.token_with_groups(&[]);
    assert_eq!(gateway.list_keys(Some(&groupless)).await, 403);

    assert_eq!(gateway.list_keys(Some(ADMIN_TOKEN)).await, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_falls_back_to_userinfo_groups(pool: PgPool) {
    let token_of = |idp: &Idp| idp.token(&idp.issuer, AUDIENCE, 300);

    let member_idp = Idp::spawn(&["ai_gateway_admins@idm.example.test"]).await;
    let member_gateway =
        Gateway::spawn(pool.clone(), Some(member_idp.authenticator(&[ADMIN_GROUP]))).await;
    assert_eq!(
        member_gateway.list_keys(Some(&token_of(&member_idp))).await,
        200
    );

    let outsider_idp = Idp::spawn(&["grafana_admins@idm.example.test"]).await;
    let outsider_gateway =
        Gateway::spawn(pool, Some(outsider_idp.authenticator(&[ADMIN_GROUP]))).await;
    assert_eq!(
        outsider_gateway
            .list_keys(Some(&token_of(&outsider_idp)))
            .await,
        403
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_rejects_missing_and_invalid_tokens(pool: PgPool) {
    let idp = Idp::spawn(&[ADMIN_GROUP]).await;
    let gateway = Gateway::spawn(pool, Some(idp.authenticator(&[ADMIN_GROUP]))).await;

    assert_eq!(gateway.list_keys(None).await, 401);
    assert_eq!(gateway.list_keys(Some("not-a-jwt")).await, 401);

    let rejected = [
        idp.token("http://other-issuer.test", AUDIENCE, 300),
        idp.token(&idp.issuer, "another-client", 300),
        idp.token(&idp.issuer, AUDIENCE, -3600),
    ];

    for token in rejected {
        assert_eq!(gateway.list_keys(Some(&token)).await, 401);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_rejects_jwts_when_oidc_is_not_configured(pool: PgPool) {
    let idp = Idp::spawn(&[ADMIN_GROUP]).await;
    let gateway = Gateway::spawn(pool, None).await;

    let token = idp.token(&idp.issuer, AUDIENCE, 300);
    assert_eq!(gateway.list_keys(Some(&token)).await, 401);
    assert_eq!(gateway.list_keys(Some(ADMIN_TOKEN)).await, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_is_grouped_by_key_and_model_within_the_window(pool: PgPool) {
    record_usage(&pool, "alpha", "model-a", "1 hour", false).await;
    record_usage(&pool, "alpha", "model-a", "2 hours", true).await;
    record_usage(&pool, "alpha", "model-b", "3 hours", false).await;
    record_usage(&pool, "beta", "model-a", "3 days", false).await;
    record_usage(&pool, "beta", "model-a", "30 days", false).await;

    let gateway = Gateway::spawn(pool, None).await;

    let (status, day) = gateway.usage("?since=24h").await;
    assert_eq!(status, 200);
    assert_eq!(
        day,
        json!([
            {
                "key_name": "alpha",
                "model": "model-a",
                "requests": 2,
                "cache_hits": 1,
                "input_tokens": 200,
                "output_tokens": 20,
                "cost_usd": 1.0,
            },
            {
                "key_name": "alpha",
                "model": "model-b",
                "requests": 1,
                "cache_hits": 0,
                "input_tokens": 100,
                "output_tokens": 10,
                "cost_usd": 0.5,
            },
        ])
    );

    let (_, default_window) = gateway.usage("").await;
    let (_, week) = gateway.usage("?since=7d").await;
    assert_eq!(default_window, week);
    assert_eq!(week.as_array().unwrap().len(), 3);
    assert_eq!(week[2]["key_name"], "beta");
    assert_eq!(week[2]["requests"], 1);

    let (_, quarter) = gateway.usage("?since=12w").await;
    assert_eq!(quarter[2]["requests"], 2);

    let (_, recent) = gateway.usage("?since=30m").await;
    assert_eq!(recent, json!([]));
}

#[sqlx::test(migrations = "./migrations")]
async fn usage_rejects_a_malformed_window(pool: PgPool) {
    let gateway = Gateway::spawn(pool, None).await;

    let (status, body) = gateway.usage("?since=soon").await;

    assert_eq!(status, 400);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid window")
    );
}
