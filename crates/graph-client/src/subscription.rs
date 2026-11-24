use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::{Context as _, bail};
use cynic::{GraphQlResponse, StreamingOperation};
use futures::{SinkExt, Stream, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use crate::{Error, GraphClient};

const PROTOCOL: &str = "graphql-transport-ws";

/// Events of one subscription; the stream ends when the server completes it.
pub struct Subscription<T> {
    events: mpsc::Receiver<anyhow::Result<T>>,
}

impl<T> Stream for Subscription<T> {
    type Item = anyhow::Result<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

fn websocket_url(url: &str) -> String {
    match url.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some(("http", rest)) => format!("ws://{rest}"),
        _ => url.to_owned(),
    }
}

pub(crate) async fn open<T, V>(
    client: &GraphClient,
    operation: StreamingOperation<T, V>,
) -> anyhow::Result<Subscription<T>>
where
    T: DeserializeOwned + Send + 'static,
    V: Serialize,
{
    let url = websocket_url(client.url());
    let mut request = url.as_str().into_client_request()?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", HeaderValue::from_static(PROTOCOL));

    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .with_context(|| format!("connecting to {url}"))?;

    let token = client.token().await?;
    let init = json!({
        "type": "connection_init",
        "payload": { "Authorization": format!("Bearer {token}") },
    });
    socket.send(Message::text(init.to_string())).await?;

    loop {
        let Some(message) = socket.next().await else {
            bail!("{url} closed before acknowledging the connection");
        };
        let message: Value = match message? {
            Message::Text(text) => serde_json::from_str(&text)?,
            Message::Close(frame) => bail!(
                "{url} refused the connection: {}",
                frame.map(|f| f.reason.to_string()).unwrap_or_default()
            ),
            _ => continue,
        };
        match message["type"].as_str() {
            Some("connection_ack") => break,
            Some("ping") => {
                socket
                    .send(Message::text(json!({ "type": "pong" }).to_string()))
                    .await?;
            }
            _ => bail!("unexpected {message} before connection_ack"),
        }
    }

    let subscribe = json!({ "id": "1", "type": "subscribe", "payload": operation });
    socket.send(Message::text(subscribe.to_string())).await?;

    let (sender, events) = mpsc::channel(32);
    tokio::spawn(async move {
        while let Some(message) = socket.next().await {
            let message: Value = match message {
                Ok(Message::Text(text)) => match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(e) => {
                        let _ = sender.send(Err(e.into())).await;
                        return;
                    }
                },
                Ok(Message::Close(_)) => return,
                Ok(_) => continue,
                Err(e) => {
                    let _ = sender.send(Err(e.into())).await;
                    return;
                }
            };

            let event = match message["type"].as_str() {
                Some("next") => {
                    match serde_json::from_value::<GraphQlResponse<T, Value>>(
                        message["payload"].clone(),
                    ) {
                        Ok(response) => match response.errors.as_ref().and_then(|e| e.first()) {
                            Some(error) => Err(Error::from(error).into()),
                            None => response
                                .data
                                .ok_or_else(|| anyhow::anyhow!("subscription event had no data")),
                        },
                        Err(e) => Err(e.into()),
                    }
                }
                Some("error") => Err(anyhow::anyhow!(
                    "subscription failed: {}",
                    message["payload"]
                )),
                Some("complete") => return,
                Some("ping") => {
                    let _ = socket
                        .send(Message::text(json!({ "type": "pong" }).to_string()))
                        .await;
                    continue;
                }
                _ => continue,
            };

            let failed = event.is_err();
            if sender.send(event).await.is_err() || failed {
                return;
            }
        }
    });

    Ok(Subscription { events })
}

#[cfg(test)]
mod tests {
    #[test]
    fn websocket_urls_follow_the_http_scheme() {
        assert_eq!(
            super::websocket_url("https://graph.ortzi.cloud/v1/graphql"),
            "wss://graph.ortzi.cloud/v1/graphql"
        );
        assert_eq!(
            super::websocket_url("http://127.0.0.1:4000/v1/graphql"),
            "ws://127.0.0.1:4000/v1/graphql"
        );
    }
}
