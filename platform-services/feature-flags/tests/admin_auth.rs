mod common;

use common::{connect_admin, connect_eval, eval_request, spawn_server_with_auth};
use feature_flags::auth::Authenticator;
use feature_flags::pb;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const KID: &str = "test-key";
const AUDIENCE: &str = "feature-flags";

struct Idp {
    issuer: String,
    key: EncodingKey,
    handle: JoinHandle<()>,
}

impl Idp {
    async fn spawn(username: &'static str) -> Self {
        let secret = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
        let key = EncodingKey::from_ec_der(secret.to_pkcs8_der().unwrap().as_bytes());

        let mut jwk: serde_json::Value =
            serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(KID);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");

        let jwks = json!({ "keys": [jwk] }).to_string();
        let userinfo = json!({ "preferred_username": username }).to_string();

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

    fn authenticator(&self) -> Authenticator {
        Authenticator::new(&self.issuer, Some(AUDIENCE.to_owned()), None, None)
    }

    fn token(&self, issuer: &str, audience: &str, expires_in: i64) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(KID.to_owned());

        let claims = json!({
            "iss": issuer,
            "aud": audience,
            "sub": "00000000-0000-0000-0000-000000000001",
            "jti": "token-1",
            "exp": chrono::Utc::now().timestamp() + expires_in,
        });

        encode(&header, &claims, &self.key).unwrap()
    }
}

fn with_bearer<T>(message: T, token: &str) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

fn create_flag(key: &str) -> pb::CreateFlagRequest {
    pb::CreateFlagRequest {
        key: key.to_owned(),
        value_type: pb::ValueType::Boolean as i32,
        enabled: true,
        default_variant_key: "on".to_owned(),
        variants: vec![pb::Variant {
            key: "on".to_owned(),
            value: Some(prost_types::Value {
                kind: Some(prost_types::value::Kind::BoolValue(true)),
            }),
        }],
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_rejects_missing_and_invalid_tokens(pool: PgPool) {
    let idp = Idp::spawn("anurag").await;
    let (endpoint, server) = spawn_server_with_auth(pool, Some(idp.authenticator())).await;
    let mut admin = connect_admin(&endpoint).await;

    let missing = admin
        .list_flags(pb::ListFlagsRequest::default())
        .await
        .expect_err("no token must be rejected");
    assert_eq!(missing.code(), tonic::Code::Unauthenticated);

    let garbage = admin
        .list_flags(with_bearer(pb::ListFlagsRequest::default(), "not-a-jwt"))
        .await
        .expect_err("garbage token must be rejected");
    assert_eq!(garbage.code(), tonic::Code::Unauthenticated);

    let rejected = [
        idp.token("http://other-issuer.test", AUDIENCE, 300),
        idp.token(&idp.issuer, "another-client", 300),
        idp.token(&idp.issuer, AUDIENCE, -3600),
    ];

    for token in rejected {
        let err = admin
            .list_flags(with_bearer(pb::ListFlagsRequest::default(), &token))
            .await
            .expect_err("invalid token must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    let spoofed = {
        let mut request = tonic::Request::new(create_flag("spoofed"));
        request
            .metadata_mut()
            .insert("actor", "mallory".parse().unwrap());
        request
    };
    let err = admin
        .create_flag(spoofed)
        .await
        .expect_err("actor header must not bypass auth");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    server.abort();
    idp.handle.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_accepts_valid_token_and_audits_the_username(pool: PgPool) {
    let idp = Idp::spawn("anurag").await;
    let (endpoint, server) = spawn_server_with_auth(pool, Some(idp.authenticator())).await;
    let mut admin = connect_admin(&endpoint).await;
    let token = idp.token(&idp.issuer, AUDIENCE, 300);

    let mut request = with_bearer(create_flag("authed"), &token);
    request
        .metadata_mut()
        .insert("actor", "mallory".parse().unwrap());
    admin.create_flag(request).await.unwrap();

    let changes = admin
        .list_changes(with_bearer(pb::ListChangesRequest::default(), &token))
        .await
        .unwrap()
        .into_inner()
        .changes;

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].actor, "anurag");

    server.abort();
    idp.handle.abort();
}

#[sqlx::test(migrations = "./migrations")]
async fn evaluation_stays_open_when_auth_is_enabled(pool: PgPool) {
    let idp = Idp::spawn("anurag").await;
    let (endpoint, server) = spawn_server_with_auth(pool, Some(idp.authenticator())).await;
    let mut evaluation = connect_eval(&endpoint).await;

    let resolved = evaluation
        .resolve_boolean(eval_request(pb::ResolveRequest {
            flag_key: "anything".to_owned(),
            context: None,
        }))
        .await;
    assert!(resolved.is_ok());

    let all = evaluation
        .resolve_all(eval_request(pb::ResolveAllRequest { context: None }))
        .await;
    assert!(all.is_ok());

    let snapshot = evaluation
        .get_snapshot(eval_request(pb::GetSnapshotRequest {}))
        .await;
    assert!(snapshot.is_ok());

    server.abort();
    idp.handle.abort();
}
