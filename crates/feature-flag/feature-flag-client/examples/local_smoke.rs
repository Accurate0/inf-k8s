//! Manual smoke test for local (in-process) evaluation against a running backend.
//! Run the service, then: `cargo run -p feature-flag-client --example local_smoke`.

use feature_flag_client::{Context, EvaluationMode, FeatureFlagClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client =
        FeatureFlagClient::connect_with("http://localhost:50051", EvaluationMode::Local).await?;

    let beta = client
        .resolve_bool("new-checkout", Context::new("u1").string("email", "a@anurag.sh"))
        .await?;
    println!(
        "beta  -> value={} variant={} reason={:?} err={:?}",
        beta.value, beta.variant, beta.reason, beta.error_code
    );

    let other = client
        .resolve_bool("new-checkout", Context::new("u2").string("email", "a@other.com"))
        .await?;
    println!(
        "other -> value={} variant={} reason={:?} err={:?}",
        other.value, other.variant, other.reason, other.error_code
    );

    let missing = client.resolve_bool("does-not-exist", Context::new("u3")).await?;
    println!("missing -> err={:?}", missing.error_code);

    Ok(())
}
