//! No-provider-spend workflow baseline, deliberately separate from `just verify`.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "e2e/harness.rs"]
mod harness;
#[allow(dead_code)]
#[path = "e2e/scripted.rs"]
mod scripted;
#[path = "quality_baseline/baseline.rs"]
mod baseline;
#[path = "quality_baseline/scenarios.rs"]
mod scenarios;

#[test]
fn record_schema_is_closed_and_secret_free() {
    baseline::schema_contract();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lane_a_repeats_have_identical_verdicts() {
    scenarios::run().await;
}
