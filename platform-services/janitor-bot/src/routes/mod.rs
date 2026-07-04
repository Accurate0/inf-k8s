mod admin;
mod argocd;
mod dashboard;
mod forgejo;
mod github;

pub use admin::*;
pub use argocd::*;
pub use dashboard::*;
pub use forgejo::*;
pub use github::*;

use axum::http::HeaderMap;
use opentelemetry::trace::TraceContextExt;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Root span for background work spawned from a webhook handler.
///
/// Detached from the current request span (`parent: None`) so the request span
/// can close as soon as the HTTP handler returns. We add a span link back to
/// the originating request so the two traces remain navigable in Tempo without
/// the active-span leak that lets unrelated incoming requests reparent under
/// the webhook trace.
fn background_span(name: &'static str) -> Span {
    let span = tracing::info_span!(parent: None, "background", task = name, otel.name = format!("background: {name}"));

    let request_ctx = Span::current().context();
    let request_span = request_ctx.span();
    let request_sc = request_span.span_context();
    if request_sc.is_valid() {
        span.add_link(request_sc.clone());

        let bg_sc = span.context().span().span_context().clone();
        if bg_sc.is_valid() {
            Span::current().add_link(bg_sc);
        }
    }
    span
}

/// Headers worth forwarding when proxying a webhook downstream. Notably the
/// GitHub event type and signature, so the downstream service can re-validate
/// the payload against its own copy of the shared secret.
const FORWARD_HEADERS: &[&str] = &[
    "content-type",
    "user-agent",
    "x-github-event",
    "x-github-delivery",
    "x-hub-signature",
    "x-hub-signature-256",
];

fn forward_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    FORWARD_HEADERS
        .iter()
        .filter_map(|name| {
            headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .map(|v| ((*name).to_string(), v.to_string()))
        })
        .collect()
}
