use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use futures::{SinkExt, StreamExt, channel::mpsc};
use open_feature::EvaluationContext;
use tracing::{Instrument, Span, field};

use crate::{
    config::Resolved,
    error::{GatewayError, Result},
    keys::VirtualKey,
    metrics,
    providers::{
        Dialect, ModelKind, Provider, ProxyRequest, Usage,
        translate::{self, SseTranslator},
    },
    response_cache::{self, CachedResponse},
    state::AppState,
    usage::{self, UsageEvent},
};

const ENABLED_FLAG: &str = "ai-gateway-enabled";
const MODEL_OVERRIDE_FLAG: &str = "ai-gateway-model-override";
const RESPONSE_CACHE_FLAG: &str = "ai-gateway-response-cache";
const STREAM_USAGE_CAP: usize = 8 * 1024 * 1024;

/// Per-provider attempts before failing over to the next provider (1 initial + retries).
const MAX_ATTEMPTS_PER_PROVIDER: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

/// Longest `Retry-After` we'll wait in-loop; beyond this, fail over to the next provider
/// rather than stall the request behind one provider's rate limit.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(2);

/// Total deadline for a non-streaming upstream request. Streaming has no total deadline
/// and relies on the client's idle (read) timeout instead.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// HTTP statuses worth retrying/failing over on: rate limits and transient upstream
/// faults. Client errors (4xx other than 429) are returned as-is.
fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// The delay from an upstream `Retry-After` header, if present and parseable.
fn retry_after_delay(resp: &reqwest::Response) -> Option<Duration> {
    parse_retry_after(resp.headers().get("retry-after")?.to_str().ok()?)
}

/// Parses a `Retry-After` value. Supports both the delta-seconds (`30`) and HTTP-date
/// forms; a date in the past yields zero.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let secs = (when.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
    Some(Duration::from_secs(secs.max(0) as u64))
}

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

    let mut request = ProxyRequest::from_slice(&body)?;
    let requested_model = request.model()?.to_owned();
    span.record("requested_model", requested_model.as_str());

    let evaluation_context = EvaluationContext::default()
        .with_targeting_key(&key.name)
        .with_custom_field("key", key.name.clone())
        .with_custom_field("requested_model", requested_model.clone());

    if !state
        .features
        .bool_flag(ENABLED_FLAG, evaluation_context.clone(), true)
        .await
    {
        return Err(GatewayError::Disabled);
    }

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

    // The runtime flag wins as a global override; otherwise the config rules resolve the
    // model and may pin a provider or deny the request outright.
    let override_model = state
        .features
        .string_flag(MODEL_OVERRIDE_FLAG, evaluation_context.clone(), "")
        .await;
    let (resolved_model, pinned_provider) = if !override_model.is_empty() {
        (override_model, None)
    } else {
        match state.config.resolve(&key.name, &requested_model) {
            Resolved::Route { model, provider } => (model, provider),
            Resolved::Denied => return Err(GatewayError::ModelDenied(requested_model)),
        }
    };

    span.record("resolved_model", resolved_model.as_str());

    let kind = ModelKind::for_sub_path(sub_path);
    let client_dialect = Dialect::for_sub_path(sub_path);

    if resolved_model != requested_model {
        request.set_model(&resolved_model);
    }

    let candidates = match &pinned_provider {
        Some(name) => state.providers.get(name).into_iter().collect(),
        None => state.providers.providers_for_model(&resolved_model, kind),
    };

    let Some(primary) = candidates.first().cloned() else {
        return Err(GatewayError::NoProvider(resolved_model));
    };

    span.record("primary_provider", primary.name());

    let streaming = request.is_stream();
    span.record("stream", streaming);

    let mut ctx = RequestContext {
        key,
        provider: primary.clone(),
        requested_model,
        resolved_model,
    };

    let cache_key = if !streaming
        && request.is_cacheable(kind)
        && state.cache.is_some()
        && state
            .features
            .bool_flag(RESPONSE_CACHE_FLAG, evaluation_context, true)
            .await
    {
        Some(response_cache::key(
            client_dialect,
            sub_path,
            &request.to_bytes()?,
        ))
    } else {
        None
    };

    if let (Some(cache), Some(k)) = (&state.cache, &cache_key)
        && let Some(hit) = response_cache::get(cache, k).await
    {
        span.record("provider", "cache");
        let usage = Usage {
            input: hit.input_tokens,
            output: hit.output_tokens,
        };
        let status = hit.status;
        record(&state, &ctx, usage, status, started, true).await;
        return Ok(hit.into_response());
    }

    // `fallback` keeps the last retryable response so an exhausted failover still returns
    // a real upstream status rather than a synthetic error.
    let mut served: Option<(Arc<dyn Provider>, reqwest::Response)> = None;
    let mut fallback: Option<(Arc<dyn Provider>, reqwest::Response)> = None;
    let mut last_err: Option<GatewayError> = None;

    'failover: for provider in &candidates {
        let outbound = outbound_for(&request, client_dialect, provider.dialect())?;
        // Carries an upstream `Retry-After` from the previous attempt to the next sleep.
        let mut retry_after: Option<Duration> = None;
        for attempt in 0..MAX_ATTEMPTS_PER_PROVIDER {
            if attempt > 0 {
                let delay = retry_after
                    .take()
                    .unwrap_or(RETRY_BASE_DELAY * (1 << (attempt - 1)));
                tokio::time::sleep(delay).await;
            }
            let upstream_span = tracing::info_span!(
                "upstream.request",
                provider = provider.name(),
                model = %ctx.resolved_model,
                attempt = attempt + 1,
            );
            let mut request = provider.build_request(&state.http, kind, outbound.clone(), &headers);
            if !streaming {
                request = request.timeout(REQUEST_TIMEOUT);
            }

            match request.send().instrument(upstream_span).await {
                Ok(resp) if is_retryable(resp.status()) => {
                    metrics::record_upstream_error(provider.name());
                    // Honor Retry-After on 429: a short wait retries this provider, a long
                    // one abandons its remaining attempts and fails over immediately.
                    let wait = (resp.status() == StatusCode::TOO_MANY_REQUESTS)
                        .then(|| retry_after_delay(&resp))
                        .flatten();
                    fallback = Some((provider.clone(), resp));
                    match wait {
                        Some(d) if d > MAX_RETRY_AFTER => continue 'failover,
                        Some(d) => retry_after = Some(d),
                        None => {}
                    }
                }
                Ok(resp) => {
                    served = Some((provider.clone(), resp));
                    break 'failover;
                }
                Err(e) => {
                    metrics::record_upstream_error(provider.name());
                    last_err = Some(e.into());
                }
            }
        }
    }

    let (provider, response) = match served.or(fallback) {
        Some(v) => v,
        None => {
            return Err(
                last_err.unwrap_or_else(|| GatewayError::NoProvider(ctx.resolved_model.clone()))
            );
        }
    };

    span.record("provider", provider.name());
    ctx.provider = provider.clone();

    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/json"));

    if streaming {
        Ok(stream_response(
            state,
            ctx,
            client_dialect,
            status,
            content_type,
            response,
            started,
        ))
    } else {
        let bytes = response.bytes().await?;
        let usage = provider.parse_usage(&bytes);

        // Error bodies aren't in the chat/messages schema, so only successful ones are
        // translated back to the client's dialect.
        let client_bytes = if provider.dialect() == client_dialect || !status.is_success() {
            bytes.clone()
        } else {
            translate::translate_response(&bytes, provider.dialect(), client_dialect)?
        };

        if let (Some(cache), Some(k)) = (&state.cache, &cache_key)
            && status.is_success()
        {
            response_cache::put(
                cache,
                k,
                &CachedResponse {
                    status: status.as_u16(),
                    content_type: content_type
                        .to_str()
                        .unwrap_or("application/json")
                        .to_owned(),
                    body: client_bytes.to_vec(),
                    input_tokens: usage.input,
                    output_tokens: usage.output,
                },
            )
            .await;
        }

        record(&state, &ctx, usage, status.as_u16(), started, false).await;

        Ok(Response::builder()
            .status(status)
            .header("content-type", content_type)
            .header(
                "x-cache",
                if cache_key.is_some() {
                    "MISS"
                } else {
                    "BYPASS"
                },
            )
            .body(Body::from(client_bytes))
            .unwrap())
    }
}

/// Serializes the request body in the dialect `provider` expects, translating from the
/// client's dialect when they differ.
fn outbound_for(request: &ProxyRequest, client: Dialect, provider: Dialect) -> Result<Bytes> {
    if client == provider {
        request.to_bytes()
    } else {
        translate::translate_request(&request.to_bytes()?, client, provider)
    }
}

/// Status recorded for a stream the client abandoned before it completed.
const CLIENT_CLOSED: u16 = 499;

fn stream_response(
    state: AppState,
    ctx: RequestContext,
    client_dialect: Dialect,
    status: StatusCode,
    content_type: HeaderValue,
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

    let provider_dialect = ctx.provider.dialect();

    tokio::spawn(
        async move {
            let mut stream = upstream.bytes_stream();
            let mut translator =
                SseTranslator::new(provider_dialect, client_dialect, &ctx.resolved_model);

            // Provider-native bytes, kept for usage parsing regardless of translation.
            let mut raw: Vec<u8> = Vec::new();

            let mut truncated = false;
            let mut aborted = false;
            let mut client_gone = false;

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(b) => {
                        if raw.len() < STREAM_USAGE_CAP {
                            raw.extend_from_slice(&b);
                            truncated = raw.len() >= STREAM_USAGE_CAP;
                        }
                        let out = translator.push(&b);
                        if !out.is_empty() && tx.send(Ok(Bytes::from(out))).await.is_err() {
                            client_gone = true;
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(std::io::Error::other(e))).await;
                        aborted = true;
                        break;
                    }
                }
            }

            if !aborted && !client_gone {
                let tail = translator.finish();
                if !tail.is_empty() {
                    let _ = tx.send(Ok(Bytes::from(tail))).await;
                }
            }

            let outcome = if aborted {
                metrics::record_upstream_error(ctx.provider.name());
                StatusCode::BAD_GATEWAY.as_u16()
            } else if client_gone {
                CLIENT_CLOSED
            } else {
                status.as_u16()
            };

            if truncated {
                metrics::record_stream_truncated(ctx.provider.name());
                tracing::warn!(
                    provider = ctx.provider.name(),
                    model = %ctx.resolved_model,
                    "stream exceeded usage buffer cap; recorded token counts may undercount"
                );
            }

            let usage = ctx.provider.parse_stream_usage(&raw);
            record(&state, &ctx, usage, outcome, started, false).await;
        }
        .instrument(stream_span),
    );

    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(Body::from_stream(rx))
        .unwrap()
}

async fn record(
    state: &AppState,
    ctx: &RequestContext,
    usage: Usage,
    status: u16,
    started: Instant,
    cache_hit: bool,
) {
    let elapsed = started.elapsed();
    let cost_usd = if cache_hit {
        0.0
    } else {
        state.pricing.cost(&ctx.resolved_model, usage)
    };

    let span = Span::current();
    span.record("upstream.status", status);
    span.record("input_tokens", usage.input);
    span.record("output_tokens", usage.output);

    tracing::info!(
        key = %ctx.key.name,
        provider = ctx.provider.name(),
        requested_model = %ctx.requested_model,
        resolved_model = %ctx.resolved_model,
        status,
        input_tokens = usage.input,
        output_tokens = usage.output,
        cost_usd,
        cache_hit,
        latency_ms = elapsed.as_millis(),
        "request completed"
    );

    metrics::record_request(
        &ctx.key.name,
        &ctx.resolved_model,
        status,
        usage.input.max(0) as u64,
        usage.output.max(0) as u64,
        elapsed,
    );
    metrics::record_cost(&ctx.key.name, &ctx.resolved_model, cost_usd);
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
            latency_ms: elapsed.as_millis() as i64,
            status: status as i32,
            cost_usd,
            cache_hit,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_delta_seconds_retry_after() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  0 "), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("garbage"), None);
    }

    #[test]
    fn parses_http_date_retry_after() {
        // A date far in the past resolves to zero rather than a negative wait.
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(Duration::ZERO)
        );
        // A date well in the future yields a positive wait.
        let future = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc2822();
        let delay = parse_retry_after(&future).unwrap();
        assert!(delay > Duration::from_secs(60), "got {delay:?}");
    }
}
