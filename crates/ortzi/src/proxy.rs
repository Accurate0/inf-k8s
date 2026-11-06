//! Loopback reverse proxy for packages that front an authenticated service in the browser, such as
//! the GraphQL lab: HTTP, server-sent events and websockets go to one upstream URL with the
//! package's token, refreshed per request. Only localhost hosts and origins are answered, so no
//! other site can use the token through it.

use std::{convert::Infallible, sync::Arc};

use bytes::Bytes;
use futures::{SinkExt, StreamExt, TryStreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{
    Request, Response, StatusCode,
    body::{Frame, Incoming},
    header::{self, HeaderMap, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use registry_api::PackageAuth;
use tokio_tungstenite::{
    Connector, WebSocketStream,
    tungstenite::{
        Message, client::IntoClientRequest, handshake::derive_accept_key, protocol::Role,
    },
};

type Body = BoxBody<Bytes, std::io::Error>;

/// Request headers that only describe the hop between the browser and the proxy.
const HOP: &[&str] = &[
    "host",
    "authorization",
    "cookie",
    "origin",
    "referer",
    "accept-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

/// Response headers the proxy recomputes or must not pass on.
const HOP_RESPONSE: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
    "set-cookie",
];

pub struct Proxy {
    pub port: u16,
    pub upstream: reqwest::Url,
    pub package: String,
    pub auth: PackageAuth,
}

struct Shared {
    proxy: Proxy,
    client: reqwest::Client,
    tls: Arc<rustls::ClientConfig>,
}

/// Serves until ctrl-c.
pub fn run(proxy: Proxy) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", proxy.port))
            .await
            .map_err(|e| format!("proxy: 127.0.0.1:{}: {e}", proxy.port))?;

        let shared = Arc::new(Shared {
            proxy,
            client: reqwest::Client::builder()
                .user_agent(concat!("ortzi/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.to_string())?,
            tls: Arc::new(tls_config()?),
        });

        loop {
            let (stream, _) = tokio::select! {
                accepted = listener.accept() => accepted.map_err(|e| format!("proxy: {e}"))?,
                _ = tokio::signal::ctrl_c() => return Ok(()),
            };

            let shared = shared.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request| handle(shared.clone(), request));
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    })
}

fn tls_config() -> Result<rustls::ClientConfig, String> {
    use rustls_platform_verifier::BuilderVerifierExt;

    rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_platform_verifier()
    .map_err(|e| e.to_string())
    .map(|builder| builder.with_no_client_auth())
}

/// Requests must name this proxy as their Host (no DNS rebinding) and, when a browser sends an
/// Origin, come from a page on it.
pub fn local(headers: &HeaderMap, port: u16) -> Result<(), &'static str> {
    let allowed = [format!("localhost:{port}"), format!("127.0.0.1:{port}")];
    let header = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };

    if !allowed
        .iter()
        .any(|host| Some(host.as_str()) == header(header::HOST))
    {
        return Err("forbidden host");
    }

    match header(header::ORIGIN) {
        Some(origin)
            if !allowed
                .iter()
                .any(|host| origin == format!("http://{host}")) =>
        {
            Err("forbidden origin")
        }
        _ => Ok(()),
    }
}

/// graphql-ws carries headers in the connection_init payload, so the token goes there too.
pub fn with_token(text: &str, token: &str) -> Option<String> {
    let mut message: serde_json::Value = serde_json::from_str(text).ok()?;
    if message.get("type")?.as_str()? != "connection_init" {
        return None;
    }

    let payload = message
        .as_object_mut()?
        .entry("payload")
        .or_insert_with(|| serde_json::json!({}));
    if !payload.is_object() {
        *payload = serde_json::json!({});
    }

    let payload = payload.as_object_mut()?;
    payload.retain(|key, _| !key.eq_ignore_ascii_case("authorization"));
    payload.insert("Authorization".to_owned(), format!("Bearer {token}").into());

    serde_json::to_string(&message).ok()
}

fn text(status: StatusCode, body: impl Into<Bytes>) -> Response<Body> {
    let body = Full::new(body.into()).map_err(|never: Infallible| match never {});

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body.boxed())
        .expect("static response")
}

async fn handle(
    shared: Arc<Shared>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    if let Err(reason) = local(request.headers(), shared.proxy.port) {
        return Ok(text(StatusCode::FORBIDDEN, format!("{reason}\n")));
    }

    let token = match crate::package_auth::token(&shared.proxy.package, &shared.proxy.auth).await {
        Ok(token) => token,
        Err(e) => {
            eprintln!("proxy: no token: {e:#}");
            return Ok(text(
                StatusCode::BAD_GATEWAY,
                "no token; check your login\n",
            ));
        }
    };

    let upgrade = request
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));

    let response = if upgrade {
        websocket(shared, request, token).await
    } else {
        forward(shared, request, token).await
    };

    Ok(response.unwrap_or_else(|e| {
        eprintln!("proxy: {e}");
        text(StatusCode::BAD_GATEWAY, format!("{e}\n"))
    }))
}

async fn forward(
    shared: Arc<Shared>,
    request: Request<Incoming>,
    token: String,
) -> Result<Response<Body>, String> {
    let (parts, body) = request.into_parts();

    let mut url = shared.proxy.upstream.clone();
    url.set_query(parts.uri.query());

    let mut headers = parts.headers;
    for name in HOP {
        headers.remove(*name);
    }
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?,
    );

    let body = body
        .collect()
        .await
        .map_err(|e| format!("reading request: {e}"))?
        .to_bytes();

    let upstream = shared
        .client
        .request(parts.method, url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("upstream: {e}"))?;

    let mut response = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers() {
        if !HOP_RESPONSE.contains(&name.as_str()) {
            response = response.header(name, value);
        }
    }

    let stream = upstream
        .bytes_stream()
        .map_ok(Frame::data)
        .map_err(std::io::Error::other);

    response
        .body(BodyExt::boxed(StreamBody::new(stream)))
        .map_err(|e| e.to_string())
}

async fn websocket(
    shared: Arc<Shared>,
    mut request: Request<Incoming>,
    token: String,
) -> Result<Response<Body>, String> {
    let key = request
        .headers()
        .get(header::SEC_WEBSOCKET_KEY)
        .ok_or("websocket: missing sec-websocket-key")?
        .clone();
    let protocols = request
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .cloned();

    let mut url = shared.proxy.upstream.clone();
    url.set_query(request.uri().query());
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|_| "websocket: bad upstream url")?;

    let mut upstream_request = url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("websocket: {e}"))?;
    let upstream_headers = upstream_request.headers_mut();
    if let Some(protocols) = &protocols {
        upstream_headers.insert(header::SEC_WEBSOCKET_PROTOCOL, protocols.clone());
    }
    upstream_headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?,
    );

    let connector = Connector::Rustls(shared.tls.clone());
    let (upstream, upstream_response) = tokio_tungstenite::connect_async_tls_with_config(
        upstream_request,
        None,
        false,
        Some(connector),
    )
    .await
    .map_err(|e| format!("websocket upstream: {e}"))?;

    let mut response = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header(
            header::SEC_WEBSOCKET_ACCEPT,
            derive_accept_key(key.as_bytes()),
        );
    if let Some(protocol) = upstream_response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
    {
        response = response.header(header::SEC_WEBSOCKET_PROTOCOL, protocol);
    }

    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let upgraded = match upgrade.await {
            Ok(upgraded) => upgraded,
            Err(e) => return eprintln!("proxy: websocket upgrade: {e}"),
        };
        let client =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;

        relay(client, upstream, token).await;
    });

    response
        .body(BodyExt::boxed(
            Full::new(Bytes::new()).map_err(|never: Infallible| match never {}),
        ))
        .map_err(|e| e.to_string())
}

async fn relay<C, U>(client: WebSocketStream<C>, upstream: WebSocketStream<U>, token: String)
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut to_client, mut from_client) = client.split();
    let (mut to_upstream, mut from_upstream) = upstream.split();

    let outbound = async {
        while let Some(Ok(message)) = from_client.next().await {
            let message = match message {
                Message::Text(text) => match with_token(&text, &token) {
                    Some(rewritten) => Message::text(rewritten),
                    None => Message::Text(text),
                },
                other => other,
            };

            if to_upstream.send(message).await.is_err() {
                break;
            }
        }
        let _ = to_upstream.close().await;
    };

    let inbound = async {
        while let Some(Ok(message)) = from_upstream.next().await {
            if to_client.send(message).await.is_err() {
                break;
            }
        }
        let _ = to_client.close().await;
    };

    tokio::select! {
        _ = outbound => {}
        _ = inbound => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    header::HeaderName::from_static(name),
                    HeaderValue::from_str(value).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn only_localhost_hosts_and_origins() {
        assert!(local(&headers(&[("host", "localhost:4280")]), 4280).is_ok());
        assert!(
            local(
                &headers(&[
                    ("host", "127.0.0.1:4280"),
                    ("origin", "http://127.0.0.1:4280")
                ]),
                4280
            )
            .is_ok()
        );

        assert_eq!(local(&headers(&[]), 4280), Err("forbidden host"));
        assert_eq!(
            local(&headers(&[("host", "evil.example:4280")]), 4280),
            Err("forbidden host")
        );
        assert_eq!(
            local(
                &headers(&[
                    ("host", "localhost:4280"),
                    ("origin", "https://evil.example")
                ]),
                4280
            ),
            Err("forbidden origin")
        );
        assert_eq!(
            local(
                &headers(&[
                    ("host", "localhost:4280"),
                    ("origin", "http://localhost:3000")
                ]),
                4280
            ),
            Err("forbidden origin")
        );
    }

    #[test]
    fn connection_init_carries_the_token() {
        let rewritten: serde_json::Value = serde_json::from_str(
            &with_token(
                r#"{"type":"connection_init","payload":{"authorization":"Bearer old","x":1}}"#,
                "fresh",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            rewritten["payload"],
            serde_json::json!({ "Authorization": "Bearer fresh", "x": 1 })
        );

        let bare: serde_json::Value =
            serde_json::from_str(&with_token(r#"{"type":"connection_init"}"#, "t").unwrap())
                .unwrap();
        assert_eq!(bare["payload"]["Authorization"], "Bearer t");

        assert_eq!(with_token(r#"{"type":"subscribe","id":"1"}"#, "t"), None);
        assert_eq!(with_token("not json", "t"), None);
    }
}
