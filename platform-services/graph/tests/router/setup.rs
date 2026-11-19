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
use subgraph::{AuthConfig, Authenticator, Claims, Provider};
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
const TIMEOUT: Duration = Duration::from_secs(10);

struct Idp {
    issuer: String,
    key: EncodingKey,
    forger: EncodingKey,
    handle: JoinHandle<()>,
}

impl Idp {
    async fn spawn() -> Self {
        let secret = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
        let key = EncodingKey::from_ec_der(secret.to_pkcs8_der().unwrap().as_bytes());

        let forged = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        let forger = EncodingKey::from_ec_der(forged.to_pkcs8_der().unwrap().as_bytes());

        let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
        jwk["kid"] = json!(KID);
        jwk["alg"] = json!("ES256");
        jwk["use"] = json!("sig");
        let jwks = json!({ "keys": [jwk] }).to_string();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());

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
            issuer,
            key,
            forger,
            handle,
        }
    }

    fn sign(&self, key: &EncodingKey, groups: &[&str]) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(KID.to_owned());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({
            "iss": self.issuer,
            "aud": AUDIENCE,
            "sub": "00000000-0000-0000-0000-000000000001",
            "preferred_username": "tester",
            "groups": groups,
            "exp": now + 300,
        });

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
        json!({ "status": self.status, "body": self.body })
    }
}

pub struct Stack {
    idp: Idp,
    pub live: Live,
    router: Child,
    url: String,
    subgraph: JoinHandle<()>,
}

impl Stack {
    pub async fn spawn() -> Self {
        Self::spawn_with_rate_limit(1000).await
    }

    pub async fn spawn_with_rate_limit(requests: u32) -> Self {
        let idp = Idp::spawn().await;

        let live = Live::default();
        let schema = Schema::build(Query, EmptyMutation, SubscriptionRoot)
            .data(live.clone())
            .finish();
        let auth = Authenticator::new(AuthConfig {
            providers: vec![Provider {
                issuer: idp.issuer.clone(),
                audience: AUDIENCE.to_owned(),
            }],
            groups: vec!["graph_users".to_owned()],
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let subgraph_url = format!("http://{}", listener.local_addr().unwrap());
        let app = subgraph::router(schema, auth);
        let subgraph = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir = std::env::temp_dir().join(format!("graph-router-test-{port}"));
        std::fs::create_dir_all(&dir).unwrap();

        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
        let supergraph = std::fs::read_to_string(format!("{fixtures}/supergraph.graphql"))
            .unwrap()
            .replace("{subgraph}", &subgraph_url);
        let config = std::fs::read_to_string(format!("{fixtures}/router.yaml"))
            .unwrap()
            .replace("{issuer}", &idp.issuer)
            .replace("{requests}", &requests.to_string());
        std::fs::write(dir.join("supergraph.graphql"), supergraph).unwrap();
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
            .env("LOG_LEVEL", "error")
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        let stack = Self {
            idp,
            live,
            router,
            url: format!("127.0.0.1:{port}"),
            subgraph,
        };
        stack.wait_ready().await;
        stack
    }

    async fn wait_ready(&self) {
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        while tokio::time::Instant::now() < deadline {
            let ready = client
                .get(format!("http://{}/readiness", self.url))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if ready {
                return;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        panic!("router did not become ready");
    }

    pub fn user(&self) -> String {
        self.idp.sign(&self.idp.key, &["graph_users"])
    }

    pub fn tester(&self) -> String {
        self.idp
            .sign(&self.idp.key, &["graph_users", "graph_testers"])
    }

    pub fn forged(&self) -> String {
        self.idp
            .sign(&self.idp.forger, &["graph_users", "graph_testers"])
    }

    pub async fn post(&self, token: Option<&str>, query: &str) -> Response {
        self.post_with_request_id(token, query, None).await
    }

    pub async fn post_document(&self, token: Option<&str>, id: &str) -> Response {
        self.send(token, json!({ "documentId": id }), None).await
    }

    pub async fn post_with_request_id(
        &self,
        token: Option<&str>,
        query: &str,
        request_id: Option<&str>,
    ) -> Response {
        self.send(token, json!({ "query": query }), request_id)
            .await
    }

    async fn send(&self, token: Option<&str>, body: Value, request_id: Option<&str>) -> Response {
        let mut request = reqwest::Client::new()
            .post(format!("http://{}/v1/graphql", self.url))
            .json(&body);

        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(request_id) = request_id {
            request = request.header("x-request-id", request_id);
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
        self.subgraph.abort();
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
            messages.push(message);

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
