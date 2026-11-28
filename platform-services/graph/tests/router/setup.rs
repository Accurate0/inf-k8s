use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::{SinkExt, Stream, StreamExt, stream};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{Value, json};
use subgraph::async_graphql::{
    Context, EmptyMutation, Error, ID, Object, Result, Schema, SimpleObject, Subscription,
};
use subgraph::{AuthConfig, Authenticator, Claims};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const KID: &str = "test-key";
const AUDIENCE: &str = "graph";
const FORGEJO_KID: &str = "forgejo-key";
const FORGEJO_AUDIENCE: &str = "graph.ortzi.test";
const TIMEOUT: Duration = Duration::from_secs(10);
const KANIDM_PATH: &str = "/oauth2/openid";
const PASSPORT_SEED: &str = "graph-router-test-passport-seed-0123456789";

struct Idp {
    base: String,
    issuer: String,
    kid: &'static str,
    key: EncodingKey,
    forger: EncodingKey,
    handle: JoinHandle<()>,
}

impl Idp {
    async fn spawn(seed: u8, kid: &'static str, client: Option<&str>) -> Self {
        let secret = p256::SecretKey::from_slice(&[seed; 32]).unwrap();
        let key = EncodingKey::from_ec_der(secret.to_pkcs8_der().unwrap().as_bytes());

        let forged = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        let forger = EncodingKey::from_ec_der(forged.to_pkcs8_der().unwrap().as_bytes());

        let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(kid);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");
        let jwks = json!({ "keys": [jwk] }).to_string();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}{KANIDM_PATH}", listener.local_addr().unwrap());
        let issuer = match client {
            Some(client) => format!("{base}/{client}"),
            None => format!("http://{}", listener.local_addr().unwrap()),
        };

        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };

                let mut buffer = vec![0u8; 8192];
                let _ = socket.read(&mut buffer).await;

                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{jwks}",
                    jwks.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        Self {
            base,
            issuer,
            kid,
            key,
            forger,
            handle,
        }
    }

    fn sign(&self, key: &EncodingKey, groups: &[&str]) -> String {
        self.sign_scoped(key, groups, "openid email profile")
    }

    fn sign_scoped(&self, key: &EncodingKey, groups: &[&str], scope: &str) -> String {
        self.sign_claims(
            key,
            json!({
                "aud": AUDIENCE,
                "sub": "00000000-0000-0000-0000-000000000001",
                "preferred_username": "tester",
                "groups": groups,
                "scope": scope,
            }),
        )
    }

    fn sign_claims(&self, key: &EncodingKey, mut claims: Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.to_owned());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        claims["iss"] = json!(self.issuer);
        claims["exp"] = json!(now + 300);

        encode(&header, &claims, key).unwrap()
    }
}

impl Drop for Idp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(SimpleObject)]
struct Tick {
    n: i32,
    user: String,
}

#[derive(SimpleObject)]
struct OpenThing {
    id: ID,
    name: String,
}

struct Query;

#[Object]
impl Query {
    async fn whoami(&self, ctx: &Context<'_>) -> String {
        ctx.data_unchecked::<Claims>().identity().to_owned()
    }

    async fn fail(&self) -> Result<String> {
        Err(Error::new("subgraph exploded"))
    }

    async fn slow(&self, ms: u64) -> bool {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        true
    }

    async fn tester_note(&self) -> Option<&str> {
        Some("for testers")
    }

    async fn secret(&self) -> Option<&str> {
        Some("hunter2")
    }

    #[graphql(entity)]
    async fn thing_by_id(&self, id: ID) -> OpenThing {
        let name = format!("thing {}", id.rsplit(':').next().unwrap_or_default());
        OpenThing { id, name }
    }
}

#[derive(Clone, Default)]
pub struct Live(Arc<AtomicUsize>);

impl Live {
    pub fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct LiveGuard(Arc<AtomicUsize>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn ticks(
    ctx: &Context<'_>,
    count: i32,
    every_ms: i32,
    fail_after: Option<i32>,
) -> impl Stream<Item = Result<Tick>> + use<> {
    let user = ctx.data_unchecked::<Claims>().identity().to_owned();
    let live = ctx.data_unchecked::<Live>().0.clone();
    live.fetch_add(1, Ordering::SeqCst);
    let guard = LiveGuard(live);

    stream::unfold((1, guard), move |(n, guard)| {
        let user = user.clone();
        async move {
            if n > count {
                return None;
            }

            tokio::time::sleep(Duration::from_millis(every_ms as u64)).await;

            if fail_after.is_some_and(|after| n > after) {
                return Some((Err(Error::new("stream exploded")), (count + 1, guard)));
            }

            Some((Ok(Tick { n, user }), (n + 1, guard)))
        }
    })
}

struct SubscriptionRoot;

#[Subscription]
impl SubscriptionRoot {
    async fn ticks(
        &self,
        ctx: &Context<'_>,
        count: i32,
        every_ms: i32,
    ) -> impl Stream<Item = Result<Tick>> {
        ticks(ctx, count, every_ms, None)
    }

    async fn failing_ticks(
        &self,
        ctx: &Context<'_>,
        after: i32,
    ) -> impl Stream<Item = Result<Tick>> {
        ticks(ctx, after + 1, 10, Some(after))
    }

    async fn secret_ticks(
        &self,
        ctx: &Context<'_>,
        count: i32,
        every_ms: i32,
    ) -> impl Stream<Item = Result<Tick>> {
        ticks(ctx, count, every_ms, None)
    }
}

pub struct Response {
    pub status: u16,
    pub request_id: Option<String>,
    pub diagnostics: Option<Value>,
    pub body: Value,
}

impl Response {
    pub fn snapshot(&self) -> Value {
        sorted(json!({ "status": self.status, "body": self.body }))
    }
}

// keys in order, whether or not serde_json preserves insertion order
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

pub struct Stack {
    idp: Idp,
    forgejo: Idp,
    pub live: Live,
    router: Child,
    url: String,
    dir: std::path::PathBuf,
    subgraphs: Vec<JoinHandle<()>>,
}

pub const BROKEN_SUPERGRAPH: &str = "type Query {";

impl Stack {
    pub async fn spawn() -> Self {
        Self::spawn_with_rate_limit(1000).await
    }

    pub async fn spawn_with_rate_limit(requests: u32) -> Self {
        Self::spawn_with_limits(requests, 1_000_000).await
    }

    pub async fn spawn_with_budget(budget: u64) -> Self {
        Self::spawn_with_limits(1000, budget).await
    }

    async fn spawn_with_limits(requests: u32, budget: u64) -> Self {
        let stack = Self::start(requests, budget, None).await;
        stack.wait_ready().await;
        stack
    }

    /// A router whose supergraph file never loads, so it should never become ready.
    pub async fn spawn_broken() -> Self {
        Self::start(1000, 1_000_000, Some(BROKEN_SUPERGRAPH)).await
    }

    pub fn replace_supergraph(&self, sdl: &str) {
        std::fs::write(self.dir.join("supergraph.graphql"), sdl).unwrap();
    }

    pub async fn ready(&self) -> bool {
        reqwest::get(format!("http://{}/readiness", self.url))
            .await
            .is_ok_and(|r| r.status().is_success())
    }

    async fn start(requests: u32, budget: u64, supergraph: Option<&str>) -> Self {
        let idp = Idp::spawn(7, KID, Some(AUDIENCE)).await;
        let forgejo = Idp::spawn(11, FORGEJO_KID, None).await;

        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let jwks = format!("http://127.0.0.1:{port}{}", subgraph::passport::JWKS_PATH);

        let live = Live::default();
        let mut subgraph_urls = Vec::new();
        let mut subgraphs = Vec::new();
        for name in ["open", "gated"] {
            let schema = Schema::build(Query, EmptyMutation, SubscriptionRoot)
                .data(live.clone())
                .finish();
            let auth = Authenticator::new(AuthConfig {
                audience: name.to_owned(),
                jwks: jwks.clone(),
                grants: Default::default(),
            });

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            subgraph_urls.push(format!("http://{}", listener.local_addr().unwrap()));
            let app = subgraph::router(schema, auth);
            subgraphs.push(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
        }

        let dir = std::env::temp_dir().join(format!("graph-router-test-{port}"));
        std::fs::create_dir_all(&dir).unwrap();

        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
        let fixture = std::fs::read_to_string(format!("{fixtures}/supergraph.graphql"))
            .unwrap()
            .replace("{open}", &subgraph_urls[0])
            .replace("{gated}", &subgraph_urls[1]);
        let config = std::fs::read_to_string(format!("{fixtures}/router.yaml"))
            .unwrap()
            .replace("{issuer}", &idp.issuer)
            .replace("{kanidm}", &idp.base)
            .replace("{forgejo}", &forgejo.issuer)
            .replace("{requests}", &requests.to_string())
            .replace("{budget}", &budget.to_string());
        std::fs::write(
            dir.join("supergraph.graphql"),
            supergraph.map_or(fixture, str::to_owned),
        )
        .unwrap();
        std::fs::write(dir.join("router.yaml"), config).unwrap();
        std::fs::copy(
            format!("{fixtures}/persisted-documents.json"),
            dir.join("persisted-documents.json"),
        )
        .unwrap();

        let router = Command::new(env!("CARGO_BIN_EXE_graph-router"))
            .current_dir(&dir)
            .env("ROUTER_CONFIG_FILE_PATH", dir.join("router.yaml"))
            .env("PORT", port.to_string())
            .env("GRAPH_PASSPORT_SEED", PASSPORT_SEED)
            .env("LOG_LEVEL", "error")
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        Self {
            idp,
            forgejo,
            live,
            router,
            url: format!("127.0.0.1:{port}"),
            dir,
            subgraphs,
        }
    }

    async fn wait_ready(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        while tokio::time::Instant::now() < deadline {
            if self.ready().await {
                return;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        panic!("router did not become ready");
    }

    pub fn user(&self) -> String {
        self.idp.sign(&self.idp.key, &["graph_users"])
    }

    /// A plain user whose token also carries `scope`, as Kanidm's supplementary scope maps add it.
    pub fn scoped(&self, scope: &str) -> String {
        self.idp.sign_scoped(
            &self.idp.key,
            &["graph_users"],
            &format!("openid email profile {scope}"),
        )
    }

    pub fn tester(&self) -> String {
        self.idp
            .sign(&self.idp.key, &["graph_users", "graph_testers"])
    }

    /// A forgejo actions token for `repository` on `git_ref`.
    pub fn forgejo(&self, repository: &str, git_ref: &str) -> String {
        self.forgejo.sign_claims(
            &self.forgejo.key,
            json!({
                "aud": FORGEJO_AUDIENCE,
                "sub": format!("repo:{repository}:ref:{git_ref}"),
                "repository": repository,
                "ref": git_ref,
                "event_name": "push",
            }),
        )
    }

    pub fn forged(&self) -> String {
        self.idp
            .sign(&self.idp.forger, &["graph_users", "graph_testers"])
    }

    pub async fn get_text(&self, path: &str) -> (u16, String) {
        let response = reqwest::get(format!("http://{}{path}", self.url))
            .await
            .unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    pub async fn post(&self, token: Option<&str>, query: &str) -> Response {
        self.post_with_request_id(token, query, None).await
    }

    pub async fn post_with_headers(
        &self,
        token: Option<&str>,
        body: Value,
        headers: &[(&str, &str)],
    ) -> Response {
        self.send(token, body, headers).await
    }

    pub fn smuggled_passport(&self, audience: &str, username: &str) -> String {
        let minter =
            subgraph::passport::Minter::from_seed(PASSPORT_SEED, Duration::from_secs(60)).unwrap();
        let identity = subgraph::access::Identity {
            issuer: "kanidm".to_owned(),
            kind: subgraph::access::IdentityKind::User,
            token_issuer: self.idp.issuer.clone(),
            subject: username.to_owned(),
            username: Some(username.to_owned()),
            client: Some(AUDIENCE.to_owned()),
            scopes: Default::default(),
            groups: vec!["graph_testers".to_owned()],
            claims: Value::Null,
            expires_at: None,
        };
        minter.mint(&identity, audience).unwrap()
    }

    pub fn passport_kid(&self) -> String {
        subgraph::passport::Minter::from_seed(PASSPORT_SEED, Duration::from_secs(60))
            .unwrap()
            .kid()
            .to_owned()
    }

    pub async fn post_document(&self, token: Option<&str>, id: &str) -> Response {
        self.send(token, json!({ "documentId": id }), &[]).await
    }

    pub async fn post_document_from(&self, ip: &str, id: &str) -> Response {
        self.send(None, json!({ "documentId": id }), &[("x-real-ip", ip)])
            .await
    }

    pub async fn post_with_request_id(
        &self,
        token: Option<&str>,
        query: &str,
        request_id: Option<&str>,
    ) -> Response {
        let headers: Vec<_> = request_id
            .map(|id| ("x-request-id", id))
            .into_iter()
            .collect();
        self.send(token, json!({ "query": query }), &headers).await
    }

    async fn send(&self, token: Option<&str>, body: Value, headers: &[(&str, &str)]) -> Response {
        let mut request = reqwest::Client::new()
            .post(format!("http://{}/v1/graphql", self.url))
            .json(&body);

        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let mut body: Value = response.json().await.unwrap();
        let diagnostics = body
            .get_mut("extensions")
            .and_then(Value::as_object_mut)
            .and_then(|extensions| extensions.remove("diagnostics"));
        if body["extensions"].as_object().is_some_and(|e| e.is_empty()) {
            body.as_object_mut().unwrap().remove("extensions");
        }

        Response {
            status,
            request_id,
            diagnostics,
            body,
        }
    }

    pub async fn query(&self, token: &str, query: &str) -> Value {
        self.post(Some(token), query).await.body
    }

    pub async fn sse(&self, token: &str, query: &str) -> String {
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/graphql", self.url))
            .bearer_auth(token)
            .header("accept", "text/event-stream")
            .json(&json!({ "query": query }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        tokio::time::timeout(TIMEOUT, response.text())
            .await
            .expect("timed out reading the event stream")
            .unwrap()
    }

    pub async fn connect(&self, token: &str) -> Socket {
        let mut socket = self.upgrade().await.unwrap();
        socket
            .send(json!({ "type": "connection_init", "payload": { "Authorization": format!("Bearer {token}") } }))
            .await;
        assert_eq!(socket.recv().await["type"], "connection_ack");
        socket
    }

    pub async fn connect_with(&self, payload: Value) -> Socket {
        let mut socket = self.upgrade().await.unwrap();
        socket
            .send(json!({ "type": "connection_init", "payload": payload }))
            .await;
        assert_eq!(socket.recv().await["type"], "connection_ack");
        socket
    }

    pub async fn connect_anonymous(&self) -> Socket {
        let mut socket = self.upgrade().await.unwrap();
        socket
            .send(json!({ "type": "connection_init", "payload": {} }))
            .await;
        assert_eq!(socket.recv().await["type"], "connection_ack");
        socket
    }

    pub async fn upgrade(&self) -> Result<Socket, tungstenite::Error> {
        let mut request = format!("ws://{}/v1/graphql", self.url)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "graphql-transport-ws".parse().unwrap(),
        );

        let (ws, _) = tokio_tungstenite::connect_async(request).await?;
        Ok(Socket { ws })
    }

    pub async fn wait_live(&self, want: usize) {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        while self.live.count() != want {
            assert!(
                tokio::time::Instant::now() < deadline,
                "subgraph still serves {} subscriptions, want {want}",
                self.live.count()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        let _ = self.router.start_kill();
        for subgraph in &self.subgraphs {
            subgraph.abort();
        }
    }
}

pub struct Socket {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Socket {
    pub async fn send(&mut self, message: Value) {
        self.ws
            .send(Message::text(message.to_string()))
            .await
            .unwrap();
    }

    pub async fn subscribe(&mut self, id: &str, query: &str) {
        self.send(json!({ "id": id, "type": "subscribe", "payload": { "query": query } }))
            .await;
    }

    pub async fn recv(&mut self) -> Value {
        loop {
            let message = tokio::time::timeout(TIMEOUT, self.ws.next())
                .await
                .expect("timed out waiting for a message")
                .expect("socket ended")
                .expect("socket failed");

            let Message::Text(text) = message else {
                panic!("unexpected frame {message:?}");
            };
            let value: Value = serde_json::from_str(&text).unwrap();

            if value["type"] == "ping" {
                self.send(json!({ "type": "pong" })).await;
                continue;
            }

            return value;
        }
    }

    pub async fn messages(&mut self, id: &str) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let message = self.recv().await;
            assert_eq!(message["id"], id, "{message}");

            let done = matches!(message["type"].as_str(), Some("complete" | "error"));
            messages.push(sorted(message));

            if done {
                return messages;
            }
        }
    }

    pub async fn ticks(&mut self, id: &str) -> Vec<i64> {
        let messages = self.messages(id).await;
        let (last, ticks) = messages.split_last().unwrap();
        assert_eq!(last["type"], "complete", "{last}");

        ticks
            .iter()
            .map(|message| {
                assert_eq!(message["type"], "next", "{message}");
                assert!(message["payload"]["errors"].is_null(), "{message}");

                let tick = message["payload"]["data"]
                    .as_object()
                    .and_then(|data| data.values().next())
                    .unwrap();
                assert_eq!(tick["user"], "tester");
                tick["n"].as_i64().unwrap()
            })
            .collect()
    }

    pub async fn close(mut self) {
        self.ws.close(None).await.unwrap();

        loop {
            match tokio::time::timeout(TIMEOUT, self.ws.next())
                .await
                .expect("timed out waiting for the close handshake")
            {
                Some(Ok(Message::Close(frame))) => {
                    if let Some(frame) = frame {
                        assert_eq!(frame.code, CloseCode::Normal);
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(tungstenite::Error::ConnectionClosed)) | None => return,
                Some(Err(e)) => panic!("router dropped the socket without a close frame: {e}"),
            }
        }
    }
}
