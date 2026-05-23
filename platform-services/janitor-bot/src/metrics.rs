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

pub fn record_evaluation(event_kind: &str, rules_matched: usize, elapsed: Duration) {
    counter!("janitor_evaluations_total", "event_kind" => event_kind.to_owned()).increment(1);
    counter!("janitor_rules_matched_total", "event_kind" => event_kind.to_owned())
        .increment(rules_matched as u64);
    histogram!("janitor_evaluation_duration_seconds", "event_kind" => event_kind.to_owned())
        .record(elapsed.as_secs_f64());
}

pub fn record_action(rule: &str, action: &str, success: bool) {
    let status = if success { "success" } else { "error" };
    counter!(
        "janitor_actions_total",
        "rule" => rule.to_owned(),
        "action" => action.to_owned(),
        "status" => status,
    )
    .increment(1);
}

pub fn record_webhook(source: &str) {
    counter!("janitor_webhooks_total", "source" => source.to_owned()).increment(1);
}

pub fn record_cron_run(elapsed: Duration, prs_evaluated: usize) {
    counter!("janitor_cron_runs_total").increment(1);
    histogram!("janitor_cron_duration_seconds").record(elapsed.as_secs_f64());
    counter!("janitor_cron_prs_evaluated_total").increment(prs_evaluated as u64);
}
