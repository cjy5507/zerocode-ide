//! The two buttons on a quota-wall chip (t-39771) — RED: the window's two
//! commands reach these, and neither presses anything yet.

use super::*;

/// RED (t-39771): no retry is typed yet.
pub(crate) fn retry(
    _host: &dyn Host,
    _run_id: &str,
    worker_id: &str,
    _now_ms: i64,
) -> Result<serde_json::Value, String> {
    Err(format!(
        "t-39771 red: worker {worker_id} is not retried yet"
    ))
}

/// RED (t-39771): no handover is walked yet.
pub(crate) fn hand_over(
    _host: &dyn Host,
    _overrides: &[(String, LaunchOverride)],
    _run_id: &str,
    worker_id: &str,
    _to: Option<&str>,
    _now_ms: i64,
) -> Result<serde_json::Value, String> {
    Err(format!(
        "t-39771 red: worker {worker_id} is not handed over yet"
    ))
}
