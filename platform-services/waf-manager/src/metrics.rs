use metrics::{counter, gauge};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

pub struct Metrics;

impl Metrics {
    pub fn init() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::set_global_recorder(recorder).expect("failed to set metrics recorder");

        HANDLE.set(handle).expect("metrics already initialized");
    }

    pub fn render() -> String {
        HANDLE
            .get()
            .map(PrometheusHandle::render)
            .unwrap_or_default()
    }

    pub fn set_active_blocks(gateway: &str, count: usize) {
        gauge!("waf_manager_active_blocks", "gateway" => gateway.to_owned()).set(count as f64);
    }

    pub fn set_conflicts(count: usize) {
        gauge!("waf_manager_policy_conflicts").set(count as f64);
    }

    pub fn record_sync(success: bool) {
        let status = if success { "success" } else { "error" };
        counter!("waf_manager_syncs_total", "status" => status).increment(1);
    }

    pub fn record_block_rejected(reason: &'static str) {
        counter!("waf_manager_blocks_rejected_total", "reason" => reason).increment(1);
    }

    pub fn record_loki_error() {
        counter!("waf_manager_loki_errors_total").increment(1);
    }

    /// Split by `mode` so the Grafana panel can show what dry-run workflows would
    /// have blocked next to what active ones actually did.
    pub fn record_workflow_block(workflow: &str, mode: &'static str) {
        counter!(
            "waf_manager_workflow_blocks_total",
            "workflow" => workflow.to_owned(),
            "mode" => mode,
        )
        .increment(1);
    }

    pub fn record_workflow_evaluation(workflow: &str) {
        counter!(
            "waf_manager_workflow_evaluations_total",
            "workflow" => workflow.to_owned(),
        )
        .increment(1);
    }

    pub fn record_workflow_error(workflow: &str) {
        counter!(
            "waf_manager_workflow_errors_total",
            "workflow" => workflow.to_owned(),
        )
        .increment(1);
    }

    pub fn record_workflow_skipped(reason: &'static str) {
        counter!("waf_manager_workflow_skipped_total", "reason" => reason).increment(1);
    }

    /// How many enabled workflows exist, by mode. A workflow silently dropping to
    /// zero here means a ConfigMap did not load as intended.
    pub fn set_workflows(mode: &'static str, count: usize) {
        gauge!("waf_manager_workflows", "mode" => mode).set(count as f64);
    }

    /// Active blocks split by origin, so the dashboard can show how much of the
    /// blocklist the workflows are responsible for.
    pub fn set_blocks_by_origin(origin: &'static str, count: usize) {
        gauge!("waf_manager_blocks_by_origin", "origin" => origin).set(count as f64);
    }

    pub fn set_suppressions(count: usize) {
        gauge!("waf_manager_suppressions").set(count as f64);
    }

    pub fn record_workflow_run(duration: std::time::Duration) {
        counter!("waf_manager_workflow_runs_total").increment(1);
        gauge!("waf_manager_workflow_run_seconds").set(duration.as_secs_f64());
    }
}
