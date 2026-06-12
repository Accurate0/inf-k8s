pub mod admin;
pub mod evaluation;

pub use admin::AdminService;
pub use evaluation::EvaluationService;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

/// Per-request span for the gRPC server. Links to the caller's trace by extracting the
/// W3C `traceparent` from request metadata, and emits a line on every request so the
/// logs show which RPC was called.
pub fn grpc_span(req: &http::Request<()>) -> tracing::Span {
    let rpc = req.uri().path();

    let is_health_check = rpc == "/grpc.health.v1.Health/Check";
    let span = if is_health_check {
        tracing::trace_span!("grpc.request", rpc = rpc)
    } else {
        tracing::info_span!("grpc.request", rpc = rpc)
    };

    let parent = opentelemetry::global::get_text_map_propagator(|p| {
        p.extract(&opentelemetry_http::HeaderExtractor(req.headers()))
    });
    let _ = span.set_parent(parent);

    span.in_scope(|| {
        if is_health_check {
            tracing::trace!("grpc request")
        } else {
            tracing::debug!("grpc request")
        }
    });

    span
}
