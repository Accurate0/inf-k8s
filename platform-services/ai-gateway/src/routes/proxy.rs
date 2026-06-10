use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures::{SinkExt, StreamExt, channel::mpsc};

use crate::{
    cache::{self, CachedResponse},
    error::{GatewayError, Result},
    keys::VirtualKey,
    metrics,
    providers::{Dialect, Provider, ProxyRequest, Usage},
    state::AppState,
    usage::{self, UsageEvent},
};

const ENABLED_FLAG: &str = "ai-gateway-enabled";
const MODEL_OVERRIDE_FLAG: &str = "ai-gateway-model-override";
const PROVIDER_FLAG: &str = "ai-gateway-provider";
const CACHE_HEADER: &str = "x-aig-cache";
const STREAM_USAGE_CAP: usize = 8 * 1024 * 1024;

pub async fn messages(state: State<AppState>, headers: HeaderMap, body: Bytes) -> Result<Response> {
    proxy(state, headers, body, Dialect::Anthropic, "/v1/messages").await
}

pub async fn chat_completions(
    state: State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    proxy(state, headers, body, Dialect::OpenAiCompatible, "/chat/completions").await
}

pub async fn embeddings(
    state: State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    proxy(state, headers, body, Dialect::OpenAiCompatible, "/embeddings").await
}

/// Carries everything resolved on the request hot path through to where usage is
/// recorded, so streaming and buffered paths record identically.
struct RequestContext {
    key: VirtualKey,
    provider: Arc<dyn Provider>,
    requested_model: String,
    resolved_model: String,
}

async fn proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
    dialect: Dialect,
    sub_path: &'static str,
) -> Result<Response> {
    let started = Instant::now();

    let raw_key = bearer(&headers).ok_or(GatewayError::MissingKey)?;
    let key = state.keys.authenticate(raw_key).await?;

    if !state.features.bool_flag(ENABLED_FLAG, &key.name, true).await {
        return Err(GatewayError::Disabled);
    }

    let mut request = ProxyRequest::from_slice(&body)?;
    let requested_model = request.model()?.to_owned();
    if !key.allows(&requested_model) {
        return Err(GatewayError::ModelNotAllowed(key.name.clone(), requested_model));
    }

    let override_model = state.features.string_flag(MODEL_OVERRIDE_FLAG, &key.name, "").await;
    let resolved_model = if override_model.is_empty() {
        requested_model.clone()
    } else {
        override_model
    };

    let provider = select_provider(&state, dialect, &key.name).await?;

    let outbound = if resolved_model != requested_model {
        request.set_model(&resolved_model);
        request.to_bytes()?
    } else {
        body.clone()
    };

    let ctx = RequestContext {
        key,
        provider,
        requested_model,
        resolved_model,
    };

    let streaming = request.is_stream();
    let cache_ttl = cache_ttl(&headers).filter(|ttl| *ttl > 0);

    if !streaming
        && let (Some(_), Some(client)) = (cache_ttl, &state.cache)
    {
        let key = cache::cache_key(ctx.provider.name(), &ctx.resolved_model, &outbound);
        if let Some(hit) = client.get(&key).await {
            return Ok(serve_cache_hit(&state, &ctx, hit, started).await);
        }
    }

    let response = ctx
        .provider
        .build_request(&state.http, sub_path, outbound.clone())
        .send()
        .await
        .inspect_err(|_| metrics::record_upstream_error(ctx.provider.name()))?;

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .cloned()
        .unwrap_or_else(|| "application/json".parse().unwrap());

    if streaming {
        Ok(stream_response(state, ctx, status, content_type, response, started))
    } else {
        let bytes = response.bytes().await?;
        let usage = ctx.provider.parse_usage(&bytes);

        if let (Some(ttl), Some(client)) = (cache_ttl, &state.cache)
            && status.is_success()
        {
            let key = cache::cache_key(ctx.provider.name(), &ctx.resolved_model, &outbound);
            client
                .put(
                    &key,
                    ttl,
                    &CachedResponse {
                        status: status.as_u16(),
                        body: bytes.to_vec(),
                        input_tokens: usage.input,
                        output_tokens: usage.output,
                    },
                )
                .await;
        }

        record(&state, &ctx, usage, false, status.as_u16(), started).await;

        Ok(Response::builder()
            .status(status)
            .header("content-type", content_type)
            .body(Body::from(bytes))
            .unwrap())
    }
}

async fn select_provider(
    state: &AppState,
    dialect: Dialect,
    key_name: &str,
) -> Result<Arc<dyn Provider>> {
    let chosen = state.features.string_flag(PROVIDER_FLAG, key_name, "").await;
    if !chosen.is_empty() {
        return state
            .providers
            .get(&chosen)
            .ok_or(GatewayError::NoProvider(chosen));
    }
    state
        .providers
        .default_for(dialect)
        .ok_or_else(|| GatewayError::NoProvider("no default provider configured".into()))
}

fn stream_response(
    state: AppState,
    ctx: RequestContext,
    status: StatusCode,
    content_type: axum::http::HeaderValue,
    upstream: reqwest::Response,
    started: Instant,
) -> Response {
    let (mut tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);

    tokio::spawn(async move {
        let mut stream = upstream.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(b) => {
                    if buf.len() < STREAM_USAGE_CAP {
                        buf.extend_from_slice(&b);
                    }
                    if tx.send(Ok(b)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e))).await;
                    break;
                }
            }
        }

        let usage = ctx.provider.parse_stream_usage(&buf);
        record(&state, &ctx, usage, false, status.as_u16(), started).await;
    });

    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(Body::from_stream(rx))
        .unwrap()
}

async fn serve_cache_hit(
    state: &AppState,
    ctx: &RequestContext,
    hit: CachedResponse,
    started: Instant,
) -> Response {
    record(state, ctx, Usage::default(), true, hit.status, started).await;

    Response::builder()
        .status(StatusCode::from_u16(hit.status).unwrap_or(StatusCode::OK))
        .header("content-type", "application/json")
        .header("x-aig-cache", "hit")
        .body(Body::from(hit.body))
        .unwrap()
}

async fn record(
    state: &AppState,
    ctx: &RequestContext,
    usage: Usage,
    cache_hit: bool,
    status: u16,
    started: Instant,
) {
    let elapsed = started.elapsed();
    metrics::record_request(
        &ctx.key.name,
        &ctx.resolved_model,
        status,
        cache_hit,
        usage.input.max(0) as u64,
        usage.output.max(0) as u64,
        elapsed,
    );
    usage::record(
        &state.pool,
        &UsageEvent {
            key_id: Some(ctx.key.id),
            key_name: ctx.key.name.clone(),
            provider: ctx.provider.name().to_owned(),
            requested_model: ctx.requested_model.clone(),
            resolved_model: ctx.resolved_model.clone(),
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_hit,
            latency_ms: elapsed.as_millis() as i64,
            status: status as i32,
        },
    )
    .await;
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn cache_ttl(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CACHE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}
