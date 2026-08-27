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
}
