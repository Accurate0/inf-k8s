use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures::{SinkExt, StreamExt, channel::mpsc};
use tracing::{Instrument, Span, field};

use crate::{
    cache::{self, CachedResponse},
    error::{GatewayError, Result},
    keys::VirtualKey,
    metrics,
    providers::{Provider, ProxyRequest, Usage},
    state::AppState,
    usage::{self, UsageEvent},
};

const ENABLED_FLAG: &str = "ai-gateway-enabled";
const MODEL_OVERRIDE_FLAG: &str = "ai-gateway-model-override";
const CACHE_HEADER: &str = "x-aig-cache";
const STREAM_USAGE_CAP: usize = 8 * 1024 * 1024;

pub async fn messages(state: State<AppState>, headers: HeaderMap, body: Bytes) -> Result<Response> {
    proxy(state, headers, body, "/v1/messages").await
}

pub async fn chat_completions(
    state: State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    proxy(state, headers, body, "/chat/completions").await
}

pub async fn embeddings(
    state: State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    proxy(state, headers, body, "/embeddings").await
}

/// Carries everything resolved on the request hot path through to where usage is
/// recorded, so streaming and buffered paths record identically.
struct RequestContext {
    key: VirtualKey,
    provider: Arc<dyn Provider>,
    requested_model: String,
    resolved_model: String,
}

#[tracing::instrument(
    skip_all,
    fields(
        otel.name = format!("proxy {sub_path}"),
        key = field::Empty,
        requested_model = field::Empty,
        resolved_model = field::Empty,
        provider = field::Empty,
        stream = field::Empty,
        cache_hit = field::Empty,
        upstream.status = field::Empty,
        input_tokens = field::Empty,
        output_tokens = field::Empty,
    )
)]
async fn proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
    sub_path: &'static str,
) -> Result<Response> {
    let started = Instant::now();
    let span = Span::current();

    let raw_key = bearer(&headers).ok_or(GatewayError::MissingKey)?;
    let key = state.keys.authenticate(raw_key).await?;
    span.record("key", key.name.as_str());

    if !state
        .features
        .bool_flag(ENABLED_FLAG, &key.name, true)
        .await
    {
        return Err(GatewayError::Disabled);
    }

    let mut request = ProxyRequest::from_slice(&body)?;
    let requested_model = request.model()?.to_owned();
    span.record("requested_model", requested_model.as_str());
    if !key.allows(&requested_model) {
        return Err(GatewayError::ModelNotAllowed(
            key.name.clone(),
            requested_model,
        ));
    }

    if let Some(budget) = key.monthly_token_budget {
        let used = state.keys.month_to_date_tokens(key.id).await?;
        if used >= budget {
            return Err(GatewayError::BudgetExceeded(key.name.clone()));
        }
    }

    let override_model = state
        .features
        .string_flag(MODEL_OVERRIDE_FLAG, &key.name, "")
        .await;
    let resolved_model = if override_model.is_empty() {
        requested_model.clone()
    } else {
        override_model
    };
    span.record("resolved_model", resolved_model.as_str());

    // The (resolved) model determines the downstream provider.
    let provider = state
        .providers
        .provider_for_model(&resolved_model)
        .ok_or_else(|| GatewayError::NoProvider(resolved_model.clone()))?;
    span.record("provider", provider.name());

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
    span.record("stream", streaming);
    let cache_ttl = cache_ttl(&headers).filter(|ttl| *ttl > 0);

    if !streaming && let (Some(_), Some(client)) = (cache_ttl, &state.cache) {
        let key = cache::cache_key(ctx.provider.name(), &ctx.resolved_model, &outbound);
        if let Some(hit) = client.get(&key).await {
            return Ok(serve_cache_hit(&state, &ctx, hit, started).await);
        }
    }

    let upstream_span = tracing::info_span!(
        "upstream.request",
        provider = ctx.provider.name(),
        model = %ctx.resolved_model,
    );
    let response = ctx
        .provider
        .build_request(&state.http, sub_path, outbound.clone())
        .send()
        .instrument(upstream_span)
        .await
        .inspect_err(|_| metrics::record_upstream_error(ctx.provider.name()))?;

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .cloned()
        .unwrap_or_else(|| "application/json".parse().unwrap());

    if streaming {
        Ok(stream_response(
            state,
            ctx,
            status,
            content_type,
            response,
            started,
        ))
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

fn stream_response(
    state: AppState,
    ctx: RequestContext,
    status: StatusCode,
    content_type: axum::http::HeaderValue,
    upstream: reqwest::Response,
    started: Instant,
) -> Response {
    let (mut tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);

    // The stream outlives the request future, so give its completion an explicit span
    // parented to the current trace, matching the buffered path's recording.
    let stream_span = tracing::info_span!(
        "proxy.stream",
        key = %ctx.key.name,
        provider = ctx.provider.name(),
        model = %ctx.resolved_model,
        upstream.status = field::Empty,
        input_tokens = field::Empty,
        output_tokens = field::Empty,
    );

    tokio::spawn(
        async move {
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
        }
        .instrument(stream_span),
    );

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

    let span = Span::current();
    span.record("upstream.status", status);
    span.record("cache_hit", cache_hit);
    span.record("input_tokens", usage.input);
    span.record("output_tokens", usage.output);

    tracing::info!(
        key = %ctx.key.name,
        provider = ctx.provider.name(),
        requested_model = %ctx.requested_model,
        resolved_model = %ctx.resolved_model,
        status,
        cache_hit,
        input_tokens = usage.input,
        output_tokens = usage.output,
        latency_ms = elapsed.as_millis(),
        "request completed"
    );

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
