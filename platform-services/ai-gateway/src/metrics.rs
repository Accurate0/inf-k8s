use metrics::{counter, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::{sync::OnceLock, time::Duration};

static RECORDER_HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();

pub fn init() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::set_global_recorder(recorder).expect("failed to set metrics recorder");

    RECORDER_HANDLE
        .set(handle)
        .expect("metrics already initialized");
}

pub fn render() -> String {
    RECORDER_HANDLE
        .get()
        .expect("metrics not initialized")
        .render()
}

/// Records a completed proxy request: latency and token counters, all labelled by
/// virtual key, resolved model and status class.
pub fn record_request(
    key_name: &str,
    model: &str,
    status: u16,
    input_tokens: u64,
    output_tokens: u64,
    elapsed: Duration,
) {
    let status_class = format!("{}xx", status / 100);

    counter!(
        "ai_gateway_requests_total",
        "key" => key_name.to_owned(),
        "model" => model.to_owned(),
        "status" => status_class,
    )
    .increment(1);

    histogram!(
        "ai_gateway_request_duration_seconds",
        "key" => key_name.to_owned(),
        "model" => model.to_owned(),
    )
    .record(elapsed.as_secs_f64());

    counter!(
        "ai_gateway_input_tokens_total",
        "key" => key_name.to_owned(),
        "model" => model.to_owned(),
    )
    .increment(input_tokens);

    counter!(
        "ai_gateway_output_tokens_total",
        "key" => key_name.to_owned(),
        "model" => model.to_owned(),
    )
    .increment(output_tokens);
}

/// Records the estimated cost of a request, labelled by virtual key and resolved model.
/// Tracked in micro-USD because Prometheus counters are integers; divide by 1e6 for
/// dollars. Skipped when zero (cache hits, unpriced models) to avoid no-op series.
pub fn record_cost(key_name: &str, model: &str, cost_usd: f64) {
    let micro = (cost_usd * 1_000_000.0).round();
    if micro <= 0.0 {
        return;
    }
    counter!(
        "ai_gateway_cost_microusd_total",
        "key" => key_name.to_owned(),
        "model" => model.to_owned(),
    )
    .increment(micro as u64);
}

pub fn record_upstream_error(provider: &str) {
    counter!("ai_gateway_upstream_errors_total", "provider" => provider.to_owned()).increment(1);
}

/// A streamed response whose body exceeded the usage-parsing buffer cap, so its recorded
/// token counts may undercount. Tracked so undercounting is visible rather than silent.
pub fn record_stream_truncated(provider: &str) {
    counter!("ai_gateway_stream_truncated_total", "provider" => provider.to_owned()).increment(1);
}
