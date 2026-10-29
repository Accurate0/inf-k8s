use metrics::counter;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::OnceLock;

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
        .map(|h| h.render())
        .unwrap_or_default()
}

pub fn record_download(ecosystem: &'static str, package: &str, target: &str) {
    counter!(
        "registry_downloads_total",
        "ecosystem" => ecosystem,
        "package" => package.to_owned(),
        "target" => target.to_owned()
    )
    .increment(1);
}

pub fn record_publish(ecosystem: &'static str, package: &str, kind: &'static str) {
    counter!(
        "registry_publishes_total",
        "ecosystem" => ecosystem,
        "package" => package.to_owned(),
        "publisher_kind" => kind
    )
    .increment(1);
}
