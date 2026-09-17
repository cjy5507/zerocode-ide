//! The sole run-record schema; Grid/Limits are read from the shared table.
use std::collections::BTreeSet;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

macro_rules! fields {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
fields!(Build { git_sha: Option<String>, dirty: Option<bool>, profile: String });
fields!(Events { name: String, major: u64, minor: u64 });
fields!(Protocol { events: Option<Events>, capabilities: Vec<String> });
fields!(Model { requested: String, effective: String, effort: Option<String> });
fields!(Account { provider: String, label: String });
fields!(Host { os: String, cpu: String, mem: Option<u64> });
fields!(Stream { queue_max: Option<u64>, paint_p95_ms: Option<f64> });
fields!(Tokens { r#in: u64, out: u64, cached: u64 });
fields!(Fields {
    task: String, surface: String, build: Build, protocol: Protocol,
    model: Model, account: Account, host: Host, verified: bool,
    interventions: u64, elapsed_ms: u64, first_visible_ms: Option<f64>,
    stream: Stream, tokens: Tokens, cost_usd: Option<f64>,
    rss_peak_mb: Option<f64>, disk_delta_mb: f64,
    lane: Lane, seed: u64, repeat: u64, reason: Option<String>
});
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane { Deterministic, Real }
fields!(Grid { requested: String, effort: Option<String> });
fields!(Limits {
    default_repeat: u64, max_repeat: u64, timeout_ms: u64, seed: u64,
    real_spend_cap_usd: u64, deterministic_spend_cap_usd: u64, interrupt_park_seconds: u32,
    paint_percentile: u32, bytes_per_mb: u32, pane_samples: u32, pane_burst_frames: u32,
    probe_off_iterations: u64, probe_off_rounds: u32, probe_poll_ms: u64
});
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct Table {
    pub fields: Vec<String>, pub grid: Vec<Grid>, pub limits: Limits,
    pub tasks: std::collections::BTreeMap<String, Task>,
}
fields!(Task { allowed: Vec<String>, surface: String });
pub fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/quality-baseline")
}
pub fn table() -> Table {
    serde_json::from_slice(&std::fs::read(fixture().join("baseline.json")).unwrap()).unwrap()
}
impl Fields {
    pub fn new(task: &str, repeat: u64) -> Self {
        Self {
            task: task.into(), surface: table().tasks[task].surface.clone(),
            build: Build { git_sha: Some(env!("ZO_BUILD_GIT_SHA").into()), dirty: Some(env!("ZO_BUILD_DIRTY") == "true"), profile: "test".into() },
            protocol: Protocol { events: None, capabilities: vec![] },
            model: Model { requested: "claude-sonnet-4-6".into(), effective: "claude-sonnet-4-6".into(), effort: None },
            account: Account { provider: "scripted-anthropic".into(), label: "hermetic".into() },
            host: Host { os: std::env::consts::OS.into(), cpu: std::env::consts::ARCH.into(), mem: None },
            verified: false, interventions: 0, elapsed_ms: 0, first_visible_ms: None,
            stream: Stream { queue_max: None, paint_p95_ms: None }, tokens: Tokens { r#in: 0, out: 0, cached: 0 },
            cost_usd: Some(0.0), rss_peak_mb: None, disk_delta_mb: 0.0,
            lane: Lane::Deterministic, seed: table().limits.seed, repeat, reason: None,
        }
    }
    pub fn json(&self) -> String {
        let line = serde_json::to_string(self).unwrap();
        validate(&line).unwrap();
        line
    }
}
pub fn validate(line: &str) -> Result<Fields, String> {
    let row: Fields = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let lower = line.to_ascii_lowercase();
    // Credential names are forbidden as well as known credential value shapes.
    for marker in ["sk-", "bearer ", "cookie", "api_key", "access_token", "refresh_token", "authorization", "ghp_", "github_pat_", "xoxb-", "test-dummy-key", "baseline-canary"] {
        if lower.contains(marker) { return Err(format!("secret marker {marker}")); }
    }
    if !table().tasks.contains_key(&row.task) || row.repeat == 0 || row.repeat > table().limits.max_repeat {
        return Err("invalid task/repeat".into());
    }
    if matches!(row.lane, Lane::Deterministic) && row.cost_usd != Some(0.0) {
        return Err("deterministic lane must spend zero".into());
    }
    Ok(row)
}
pub fn schema_contract() {
    let value = serde_json::to_value(Fields::new("Q1", 1)).unwrap();
    if let Some(path) = std::env::var_os("BASELINE_OUT") {
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(PathBuf::from(path).join("record-template.json"), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let names: BTreeSet<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(names, table().fields.into_iter().collect());
    validate(&value.to_string()).unwrap();
    let mut extra = value.clone(); extra["account"]["token"] = json!("unexpected");
    assert!(validate(&extra.to_string()).is_err());
    for secret in ["sk-live-canary", "Bearer canary", "session_cookie", "ghp_canary", "access_token", "xoxb-canary"] {
        let mut unsafe_row = value.clone(); unsafe_row["account"]["label"] = json!(secret);
        assert!(validate(&unsafe_row.to_string()).is_err(), "{secret}");
    }
    let grid = table().grid;
    assert_eq!(grid.len(), 4);
    assert_eq!(table().limits.deterministic_spend_cap_usd, 0);
    assert!(table().limits.real_spend_cap_usd > 0);
    // Also validate records emitted by the JS window runner with this serde schema.
    if let Some(path) = std::env::var_os("BASELINE_VALIDATE") {
        for line in std::fs::read_to_string(path).unwrap().lines() { validate(line).unwrap(); }
    }
}
pub fn messages(body: &str) -> Value {
    serde_json::from_str::<Value>(body).unwrap()["messages"].clone()
}
